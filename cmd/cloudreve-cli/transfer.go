package main

import (
	"bytes"
	"context"
	"encoding/json"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"mime"
	"net/http"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"strings"
	"time"
)

const (
	maxTransferErrorSize = 1 << 20
	maxTransferAttempts  = 4
)

type uploadOptions struct {
	PolicyID  string
	MimeType  string
	Overwrite bool
}

type uploadSession struct {
	SessionID      string         `json:"session_id"`
	ChunkSize      int64          `json:"chunk_size"`
	UploadURLs     []string       `json:"upload_urls"`
	CompleteURL    string         `json:"completeURL"`
	URI            string         `json:"uri"`
	CallbackSecret string         `json:"callback_secret"`
	StoragePolicy  *storagePolicy `json:"storage_policy"`
}

type storagePolicy struct {
	Type  string `json:"type"`
	Relay bool   `json:"relay"`
}

type completeMultipartUpload struct {
	XMLName xml.Name       `xml:"CompleteMultipartUpload"`
	Parts   []completePart `xml:"Part"`
}

type completePart struct {
	PartNumber int    `xml:"PartNumber"`
	ETag       string `xml:"ETag"`
}

type downloadResult struct {
	SourceURI string `json:"source_uri"`
	Path      string `json:"path"`
	Bytes     int64  `json:"bytes"`
}

func uploadFile(ctx context.Context, api *client, localPath, destinationURI string, options uploadOptions) (result json.RawMessage, err error) {
	fileInfo, err := os.Stat(localPath)
	if err != nil {
		return nil, fmt.Errorf("stat local file: %w", err)
	}
	if !fileInfo.Mode().IsRegular() {
		return nil, errors.New("LOCAL_FILE must be a regular file")
	}

	destinationURI, err = resolveDestinationURI(destinationURI, filepath.Base(localPath))
	if err != nil {
		return nil, err
	}
	mimeType := strings.TrimSpace(options.MimeType)
	if mimeType == "" {
		mimeType = mime.TypeByExtension(strings.ToLower(filepath.Ext(localPath)))
	}
	if mimeType == "" {
		mimeType = "application/octet-stream"
	}

	requestBody := map[string]any{
		"uri":           destinationURI,
		"size":          fileInfo.Size(),
		"last_modified": fileInfo.ModTime().UnixMilli(),
		"mime_type":     mimeType,
		"policy_id":     options.PolicyID,
	}
	if options.Overwrite {
		requestBody["entity_type"] = "version"
	}
	rawSession, err := api.request(ctx, http.MethodPut, "/file/upload", requestBody)
	if err != nil {
		return nil, fmt.Errorf("create upload session: %w", err)
	}

	var session uploadSession
	if err := json.Unmarshal(rawSession, &session); err != nil {
		return nil, fmt.Errorf("decode upload session: %w", err)
	}
	if session.SessionID == "" || session.URI == "" || session.StoragePolicy == nil {
		return nil, errors.New("upload session is missing required fields")
	}
	completed := false
	defer func() {
		if completed {
			return
		}
		cleanupCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		_ = cancelUploadSession(cleanupCtx, api, &session)
	}()

	policyType := strings.ToLower(string(session.StoragePolicy.Type))
	if policyType != "oss" && policyType != "s3" {
		return nil, fmt.Errorf("unsupported storage policy type %q; CloudreveLite CLI supports only oss and s3", policyType)
	}
	if session.StoragePolicy.Relay {
		return nil, errors.New("refusing relayed upload; configure the OSS/S3 policy for direct upload")
	}
	if session.CompleteURL == "" || len(session.UploadURLs) == 0 {
		return nil, errors.New("direct upload session did not include signed upload URLs")
	}
	if policyType == "s3" && session.CallbackSecret == "" {
		return nil, errors.New("S3 upload session did not include a callback secret")
	}

	expectedChunks, err := expectedChunkCount(fileInfo.Size(), session.ChunkSize)
	if err != nil {
		return nil, err
	}
	if len(session.UploadURLs) != expectedChunks {
		return nil, fmt.Errorf("upload session returned %d URLs for %d expected chunks", len(session.UploadURLs), expectedChunks)
	}

	local, err := os.Open(localPath)
	if err != nil {
		return nil, fmt.Errorf("open local file: %w", err)
	}
	defer local.Close()

	parts := make([]completePart, 0, len(session.UploadURLs))
	for index, signedURL := range session.UploadURLs {
		offset, length := chunkRange(fileInfo.Size(), session.ChunkSize, index)
		etag, err := uploadSignedPart(ctx, api, signedURL, local, offset, length)
		if err != nil {
			return nil, fmt.Errorf("upload chunk %d: %w", index+1, err)
		}
		if policyType == "s3" {
			if strings.TrimSpace(etag) == "" {
				return nil, fmt.Errorf("upload chunk %d: S3 response did not include an ETag", index+1)
			}
			parts = append(parts, completePart{PartNumber: index + 1, ETag: etag})
		}
	}

	if err := completeSignedUpload(ctx, api, policyType, session.CompleteURL, parts); err != nil {
		return nil, fmt.Errorf("complete multipart upload: %w", err)
	}
	if policyType == "s3" {
		callbackPath := "/callback/s3/" + url.PathEscape(session.SessionID) + "/" + url.PathEscape(session.CallbackSecret)
		if _, err := api.request(ctx, http.MethodGet, callbackPath, nil); err != nil {
			return nil, fmt.Errorf("complete Cloudreve S3 callback: %w", err)
		}
	}
	completed = true

	query := url.Values{"uri": {session.URI}}
	result, err = api.request(ctx, http.MethodGet, "/file/info?"+query.Encode(), nil)
	if err != nil {
		return nil, fmt.Errorf("read uploaded file metadata: %w", err)
	}
	return result, nil
}

func resolveDestinationURI(destinationURI, fileName string) (string, error) {
	if !strings.HasSuffix(destinationURI, "/") {
		return destinationURI, nil
	}
	target, err := url.Parse(destinationURI)
	if err != nil {
		return "", fmt.Errorf("invalid destination URI: %w", err)
	}
	if !strings.EqualFold(target.Scheme, "cloudreve") || target.Host == "" {
		return "", errors.New("destination URI must use the cloudreve scheme")
	}
	target.Path = path.Join(target.Path, fileName)
	target.RawPath = ""
	return target.String(), nil
}

func expectedChunkCount(size, chunkSize int64) (int, error) {
	if size < 0 {
		return 0, errors.New("file size cannot be negative")
	}
	if chunkSize <= 0 {
		return 1, nil
	}
	count := int64(1)
	if size > 0 {
		count = (size-1)/chunkSize + 1
	}
	if count > int64(^uint(0)>>1) {
		return 0, errors.New("file requires too many upload chunks")
	}
	return int(count), nil
}

func chunkRange(size, chunkSize int64, index int) (offset, length int64) {
	if chunkSize <= 0 {
		return 0, size
	}
	offset = int64(index) * chunkSize
	length = min(chunkSize, max(0, size-offset))
	return offset, length
}

func uploadSignedPart(ctx context.Context, api *client, rawURL string, file *os.File, offset, length int64) (string, error) {
	target, err := api.resolveTransferURL(rawURL)
	if err != nil {
		return "", err
	}

	for attempt := 0; attempt < maxTransferAttempts; attempt++ {
		body := io.NewSectionReader(file, offset, length)
		req, err := http.NewRequestWithContext(ctx, http.MethodPut, target.String(), body)
		if err != nil {
			return "", err
		}
		req.ContentLength = length
		req.Header.Set("Content-Type", "application/octet-stream")
		req.Header.Set("User-Agent", "cloudreve-cli/"+version)

		resp, requestErr := api.transferHTTP.Do(req)
		if requestErr != nil {
			if attempt+1 < maxTransferAttempts {
				if err := waitForRetry(ctx, attempt); err != nil {
					return "", err
				}
				continue
			}
			return "", requestErr
		}
		raw, readErr := readLimited(resp.Body, maxTransferErrorSize)
		resp.Body.Close()
		if readErr != nil {
			return "", readErr
		}
		if resp.StatusCode >= http.StatusOK && resp.StatusCode < http.StatusMultipleChoices {
			return resp.Header.Get("ETag"), nil
		}
		if (resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= http.StatusInternalServerError) && attempt+1 < maxTransferAttempts {
			if err := waitForRetry(ctx, attempt); err != nil {
				return "", err
			}
			continue
		}
		return "", fmt.Errorf("HTTP %s: %s", resp.Status, strings.TrimSpace(string(raw)))
	}
	return "", errors.New("upload attempts exhausted")
}

func waitForRetry(ctx context.Context, attempt int) error {
	timer := time.NewTimer(time.Duration(1<<attempt) * time.Second)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

func completeSignedUpload(ctx context.Context, api *client, policyType, rawURL string, parts []completePart) error {
	target, err := api.resolveTransferURL(rawURL)
	if err != nil {
		return err
	}
	var body []byte
	if policyType == "s3" {
		body, err = xml.Marshal(completeMultipartUpload{Parts: parts})
		if err != nil {
			return err
		}
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, target.String(), bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/octet-stream")
	req.Header.Set("User-Agent", "cloudreve-cli/"+version)
	if policyType == "oss" {
		req.Header.Set("x-oss-forbid-overwrite", "true")
		req.Header.Set("x-oss-complete-all", "yes")
	}

	resp, err := api.transferHTTP.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	raw, err := readLimited(resp.Body, maxTransferErrorSize)
	if err != nil {
		return err
	}
	if resp.StatusCode < http.StatusOK || resp.StatusCode >= http.StatusMultipleChoices {
		return fmt.Errorf("HTTP %s: %s", resp.Status, strings.TrimSpace(string(raw)))
	}
	if policyType == "s3" && len(bytes.TrimSpace(raw)) > 0 {
		var root struct{ XMLName xml.Name }
		if xml.Unmarshal(raw, &root) == nil && strings.EqualFold(root.XMLName.Local, "Error") {
			return fmt.Errorf("S3 returned an error document: %s", strings.TrimSpace(string(raw)))
		}
	}
	return nil
}

func cancelUploadSession(ctx context.Context, api *client, session *uploadSession) error {
	if session.SessionID == "" || session.URI == "" {
		return nil
	}
	_, err := api.request(ctx, http.MethodDelete, "/file/upload", map[string]string{
		"id":  session.SessionID,
		"uri": session.URI,
	})
	return err
}

func downloadFile(ctx context.Context, api *client, sourceURI, localPath string, force bool) (*downloadResult, error) {
	raw, err := api.request(ctx, http.MethodPost, "/file/url", map[string]any{
		"uris":     []string{sourceURI},
		"download": true,
	})
	if err != nil {
		return nil, err
	}
	var response struct {
		URLs []struct {
			URL string `json:"url"`
		} `json:"urls"`
	}
	if err := json.Unmarshal(raw, &response); err != nil {
		return nil, fmt.Errorf("decode download URL: %w", err)
	}
	if len(response.URLs) != 1 || response.URLs[0].URL == "" {
		return nil, fmt.Errorf("expected one download URL, got %d", len(response.URLs))
	}
	target, err := api.resolveTransferURL(response.URLs[0].URL)
	if err != nil {
		return nil, err
	}

	if localPath == "" {
		localPath, err = fileNameFromURI(sourceURI)
		if err != nil {
			return nil, err
		}
	}
	absolutePath, err := filepath.Abs(localPath)
	if err != nil {
		return nil, err
	}
	if existing, statErr := os.Stat(absolutePath); statErr == nil {
		if existing.IsDir() {
			return nil, fmt.Errorf("local destination %q is a directory", absolutePath)
		}
		if !force {
			return nil, fmt.Errorf("local destination %q already exists; pass --force to replace it", absolutePath)
		}
	} else if !errors.Is(statErr, os.ErrNotExist) {
		return nil, statErr
	}

	destinationDir := filepath.Dir(absolutePath)
	if info, statErr := os.Stat(destinationDir); statErr != nil || !info.IsDir() {
		if statErr != nil {
			return nil, fmt.Errorf("destination directory: %w", statErr)
		}
		return nil, fmt.Errorf("destination parent %q is not a directory", destinationDir)
	}
	temporary, err := os.CreateTemp(destinationDir, ".cloudreve-download-*")
	if err != nil {
		return nil, err
	}
	temporaryPath := temporary.Name()
	defer os.Remove(temporaryPath)

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, target.String(), nil)
	if err != nil {
		temporary.Close()
		return nil, err
	}
	req.Header.Set("User-Agent", "cloudreve-cli/"+version)
	resp, err := api.transferHTTP.Do(req)
	if err != nil {
		temporary.Close()
		return nil, err
	}
	if resp.StatusCode < http.StatusOK || resp.StatusCode >= http.StatusMultipleChoices {
		rawError, readErr := readLimited(resp.Body, maxTransferErrorSize)
		resp.Body.Close()
		temporary.Close()
		if readErr != nil {
			return nil, readErr
		}
		return nil, fmt.Errorf("HTTP %s: %s", resp.Status, strings.TrimSpace(string(rawError)))
	}
	written, copyErr := io.Copy(temporary, resp.Body)
	closeBodyErr := resp.Body.Close()
	closeFileErr := temporary.Close()
	if copyErr != nil {
		return nil, copyErr
	}
	if closeBodyErr != nil {
		return nil, closeBodyErr
	}
	if closeFileErr != nil {
		return nil, closeFileErr
	}
	if resp.ContentLength >= 0 && written != resp.ContentLength {
		return nil, fmt.Errorf("downloaded %d bytes, expected %d", written, resp.ContentLength)
	}
	if force {
		if removeErr := os.Remove(absolutePath); removeErr != nil && !errors.Is(removeErr, os.ErrNotExist) {
			return nil, removeErr
		}
	}
	if err := os.Rename(temporaryPath, absolutePath); err != nil {
		return nil, err
	}
	return &downloadResult{SourceURI: sourceURI, Path: absolutePath, Bytes: written}, nil
}

func fileNameFromURI(sourceURI string) (string, error) {
	parsed, err := url.Parse(sourceURI)
	if err != nil {
		return "", fmt.Errorf("invalid source URI: %w", err)
	}
	name := path.Base(parsed.Path)
	if name == "." || name == "/" || name == "" {
		return "", errors.New("cannot derive a local filename from the source URI")
	}
	return name, nil
}
