package main

import (
	"archive/tar"
	"archive/zip"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"

	"github.com/spf13/cobra"
)

const (
	githubReleasesURL  = "https://api.github.com/repos/AndersonBY/CloudreveLite/releases"
	githubDownloadsURL = "https://github.com/AndersonBY/CloudreveLite/releases/download"
	maxUpdateResponse  = 4 << 20
	maxUpdateArchive   = 128 << 20
)

type githubRelease struct {
	TagName    string        `json:"tag_name"`
	Draft      bool          `json:"draft"`
	Prerelease bool          `json:"prerelease"`
	Assets     []githubAsset `json:"assets"`
}

type githubAsset struct {
	Name               string `json:"name"`
	BrowserDownloadURL string `json:"browser_download_url"`
	Size               int64  `json:"size"`
}

type availableCLIUpdate struct {
	ReleaseTag string
	Version    string
	Asset      string
	AssetURL   string
	AssetSize  int64
}

func newUpdateCommand(stdout io.Writer) *cobra.Command {
	var checkOnly bool
	cmd := &cobra.Command{
		Use:     "update",
		Aliases: []string{"upgrade"},
		Short:   "Check for and install the latest CLI release",
		Args:    cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			available, err := findCLIUpdate(cmd.Context(), runtime.GOOS, runtime.GOARCH)
			if err != nil {
				return err
			}
			if available == nil {
				return printValue(stdout, map[string]any{
					"current_version":  version,
					"update_available": false,
				})
			}
			if checkOnly {
				return printValue(stdout, map[string]any{
					"current_version":  version,
					"latest_version":   available.Version,
					"update_available": true,
					"asset":            available.Asset,
					"release_tag":      available.ReleaseTag,
				})
			}
			if err := installCLIUpdate(cmd.Context(), available); err != nil {
				return err
			}
			return printValue(stdout, map[string]any{
				"current_version": version,
				"updated_version": available.Version,
				"updated":         true,
				"asset":           available.Asset,
				"release_tag":     available.ReleaseTag,
			})
		},
	}
	cmd.Flags().BoolVar(&checkOnly, "check", false, "only check for an update")
	return cmd
}

func findCLIUpdate(ctx context.Context, goos, goarch string) (*availableCLIUpdate, error) {
	current, ok := parseCLIVersion(version)
	if !ok {
		return nil, fmt.Errorf("current CLI version %q is not a release version", version)
	}
	client := &http.Client{Timeout: 60 * time.Second}
	var releases []githubRelease
	for page := 1; page <= 10; page++ {
		request, err := http.NewRequestWithContext(ctx, http.MethodGet,
			githubReleasesURL+"?per_page=100&page="+strconv.Itoa(page), nil)
		if err != nil {
			return nil, err
		}
		request.Header.Set("Accept", "application/vnd.github+json")
		request.Header.Set("User-Agent", "cloudreve-cli/"+version)
		response, err := client.Do(request)
		if err != nil {
			return nil, fmt.Errorf("check GitHub releases: %w", err)
		}
		body, readErr := readLimited(response.Body, maxUpdateResponse)
		response.Body.Close()
		if readErr != nil {
			return nil, fmt.Errorf("read GitHub releases: %w", readErr)
		}
		if response.StatusCode < http.StatusOK || response.StatusCode >= http.StatusMultipleChoices {
			return nil, fmt.Errorf("GitHub releases returned HTTP %s: %s", response.Status, strings.TrimSpace(string(body)))
		}
		var pageReleases []githubRelease
		if err := json.Unmarshal(body, &pageReleases); err != nil {
			return nil, fmt.Errorf("decode GitHub releases: %w", err)
		}
		releases = append(releases, pageReleases...)
		if len(pageReleases) < 100 {
			break
		}
	}
	return selectCLIUpdate(releases, current, goos, goarch)
}

func parseCLIVersion(text string) ([3]uint64, bool) {
	text = strings.TrimPrefix(text, "cli-v")
	text = strings.TrimPrefix(text, "v")
	parts := strings.Split(text, ".")
	if len(parts) != 3 {
		return [3]uint64{}, false
	}
	var result [3]uint64
	for i, part := range parts {
		if part == "" || strings.Trim(part, "0123456789") != "" {
			return [3]uint64{}, false
		}
		value, err := strconv.ParseUint(part, 10, 64)
		if err != nil {
			return [3]uint64{}, false
		}
		result[i] = value
	}
	return result, true
}

func cliVersionForRelease(release githubRelease) ([3]uint64, string, bool) {
	if !release.Draft && !release.Prerelease {
		if version, ok := parseTaggedCLIVersion(release.TagName); ok {
			return version, "cli-v" + formatCLIVersion(version), true
		}
	}
	for _, asset := range release.Assets {
		const prefix = "cloudreve-cli-version-v"
		if !strings.HasPrefix(asset.Name, prefix) || !strings.HasSuffix(asset.Name, ".txt") {
			continue
		}
		text := strings.TrimSuffix(strings.TrimPrefix(asset.Name, "cloudreve-cli-version-"), ".txt")
		if version, ok := parseCLIVersion(text); ok {
			return version, "cli-v" + formatCLIVersion(version), true
		}
	}
	return [3]uint64{}, "", false
}

func parseTaggedCLIVersion(tag string) ([3]uint64, bool) {
	if !strings.HasPrefix(tag, "cli-v") {
		return [3]uint64{}, false
	}
	return parseCLIVersion(tag)
}

func formatCLIVersion(version [3]uint64) string {
	return fmt.Sprintf("%d.%d.%d", version[0], version[1], version[2])
}

func selectCLIUpdate(releases []githubRelease, current [3]uint64, goos, goarch string) (*availableCLIUpdate, error) {
	assetName := "cloudreve-cli_" + goos + "_" + goarch
	if goos == "windows" {
		assetName += ".zip"
	} else if goos == "linux" || goos == "darwin" {
		assetName += ".tar.gz"
	} else {
		return nil, fmt.Errorf("unsupported update platform %s/%s", goos, goarch)
	}

	var selected *availableCLIUpdate
	var selectedVersion [3]uint64
	for _, release := range releases {
		if release.Draft || release.Prerelease {
			continue
		}
		releaseVersion, _, ok := cliVersionForRelease(release)
		if !ok || compareCLIVersion(releaseVersion, current) <= 0 || (selected != nil && compareCLIVersion(releaseVersion, selectedVersion) <= 0) {
			continue
		}
		asset, ok := findAsset(release.Assets, assetName)
		if !ok || asset.Size <= 0 || asset.Size > maxUpdateArchive {
			continue
		}
		expectedURL := githubDownloadsURL + "/" + release.TagName + "/" + assetName
		if asset.BrowserDownloadURL != expectedURL {
			continue
		}
		selectedVersion = releaseVersion
		selected = &availableCLIUpdate{
			ReleaseTag: release.TagName,
			Version:    "cli-v" + formatCLIVersion(releaseVersion),
			Asset:      assetName,
			AssetURL:   expectedURL,
			AssetSize:  asset.Size,
		}
	}
	return selected, nil
}

func compareCLIVersion(left, right [3]uint64) int {
	for i := range left {
		if left[i] < right[i] {
			return -1
		}
		if left[i] > right[i] {
			return 1
		}
	}
	return 0
}

func findAsset(assets []githubAsset, name string) (githubAsset, bool) {
	for _, asset := range assets {
		if asset.Name == name {
			return asset, true
		}
	}
	return githubAsset{}, false
}

func installCLIUpdate(ctx context.Context, available *availableCLIUpdate) error {
	current, err := os.Executable()
	if err != nil {
		return fmt.Errorf("locate current executable: %w", err)
	}
	tempRoot := ""
	if runtime.GOOS != "windows" {
		tempRoot = filepath.Dir(current)
	}
	tempDir, err := os.MkdirTemp(tempRoot, ".cloudreve-cli-update-")
	if err != nil {
		return fmt.Errorf("create update directory: %w", err)
	}
	keepTemp := false
	defer func() {
		if !keepTemp {
			_ = os.RemoveAll(tempDir)
		}
	}()

	client := &http.Client{Timeout: 5 * time.Minute}
	archivePath := filepath.Join(tempDir, available.Asset)
	if err := downloadUpdateFile(ctx, client, available.AssetURL, archivePath, available.AssetSize); err != nil {
		return err
	}
	if info, err := os.Stat(archivePath); err != nil {
		return fmt.Errorf("stat update archive: %w", err)
	} else if info.Size() != available.AssetSize {
		return fmt.Errorf("update archive size mismatch: got %d bytes, want %d", info.Size(), available.AssetSize)
	}
	checksumsPath := filepath.Join(tempDir, "SHA256SUMS")
	checksumURL := githubDownloadsURL + "/" + available.ReleaseTag + "/SHA256SUMS"
	if err := downloadUpdateFile(ctx, client, checksumURL, checksumsPath, maxUpdateResponse); err != nil {
		return err
	}
	checksums, err := os.ReadFile(checksumsPath)
	if err != nil {
		return fmt.Errorf("read update checksums: %w", err)
	}
	expected, err := checksumForAsset(string(checksums), available.Asset)
	if err != nil {
		return err
	}
	if err := verifySHA256(archivePath, expected); err != nil {
		return err
	}

	binaryName := "cloudreve-cli"
	if runtime.GOOS == "windows" {
		binaryName += ".exe"
	}
	replacement := filepath.Join(tempDir, binaryName)
	if err := extractUpdateBinary(archivePath, binaryName, replacement); err != nil {
		return err
	}
	if err := makeExecutable(replacement); err != nil {
		return err
	}
	if runtime.GOOS == "windows" {
		if err := scheduleWindowsReplacement(current, replacement, tempDir); err != nil {
			return err
		}
		keepTemp = true
		return nil
	}
	if err := os.Rename(replacement, current); err != nil {
		return fmt.Errorf("replace current executable: %w", err)
	}
	return nil
}

func downloadUpdateFile(ctx context.Context, client *http.Client, url, path string, limit int64) error {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	request.Header.Set("Accept", "application/octet-stream")
	request.Header.Set("User-Agent", "cloudreve-cli/"+version)
	response, err := client.Do(request)
	if err != nil {
		return fmt.Errorf("download update file: %w", err)
	}
	defer response.Body.Close()
	if response.StatusCode < http.StatusOK || response.StatusCode >= http.StatusMultipleChoices {
		return fmt.Errorf("download update file returned HTTP %s", response.Status)
	}
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o755)
	if err != nil {
		return fmt.Errorf("create update file: %w", err)
	}
	_, copyErr := io.Copy(file, io.LimitReader(response.Body, limit+1))
	closeErr := file.Close()
	if copyErr != nil {
		return fmt.Errorf("save update file: %w", copyErr)
	}
	if closeErr != nil {
		return fmt.Errorf("close update file: %w", closeErr)
	}
	if info, err := os.Stat(path); err != nil {
		return err
	} else if info.Size() > limit {
		return fmt.Errorf("update file exceeds %d bytes", limit)
	}
	return nil
}

func checksumForAsset(text, assetName string) (string, error) {
	for _, line := range strings.Split(text, "\n") {
		fields := strings.Fields(line)
		if len(fields) >= 2 && (fields[1] == assetName || fields[1] == "*"+assetName) {
			if len(fields[0]) == sha256.Size*2 {
				if _, err := hex.DecodeString(fields[0]); err == nil {
					return strings.ToLower(fields[0]), nil
				}
			}
		}
	}
	return "", fmt.Errorf("no valid checksum was published for %s", assetName)
}

func verifySHA256(path, expected string) error {
	file, err := os.Open(path)
	if err != nil {
		return fmt.Errorf("open update archive: %w", err)
	}
	defer file.Close()
	hash := sha256.New()
	if _, err := io.Copy(hash, file); err != nil {
		return fmt.Errorf("hash update archive: %w", err)
	}
	if actual := hex.EncodeToString(hash.Sum(nil)); actual != expected {
		return errors.New("update archive SHA-256 verification failed")
	}
	return nil
}

func extractUpdateBinary(archivePath, binaryName, destination string) error {
	if strings.HasSuffix(archivePath, ".zip") {
		archive, err := zip.OpenReader(archivePath)
		if err != nil {
			return fmt.Errorf("open update archive: %w", err)
		}
		defer archive.Close()
		for _, entry := range archive.File {
			if entry.Name != binaryName || entry.FileInfo().IsDir() {
				continue
			}
			input, err := entry.Open()
			if err != nil {
				return err
			}
			err = copyFile(input, destination)
			input.Close()
			return err
		}
	} else {
		file, err := os.Open(archivePath)
		if err != nil {
			return fmt.Errorf("open update archive: %w", err)
		}
		defer file.Close()
		compressed, err := gzip.NewReader(file)
		if err != nil {
			return fmt.Errorf("read update archive: %w", err)
		}
		defer compressed.Close()
		reader := tar.NewReader(compressed)
		for {
			header, err := reader.Next()
			if errors.Is(err, io.EOF) {
				break
			}
			if err != nil {
				return fmt.Errorf("read update archive: %w", err)
			}
			if header.Name == binaryName && header.Typeflag == tar.TypeReg {
				return copyFile(reader, destination)
			}
		}
	}
	return fmt.Errorf("update archive does not contain %s", binaryName)
}

func copyFile(input io.Reader, destination string) error {
	output, err := os.OpenFile(destination, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o755)
	if err != nil {
		return fmt.Errorf("create replacement executable: %w", err)
	}
	if _, err := io.Copy(output, input); err != nil {
		output.Close()
		return fmt.Errorf("extract replacement executable: %w", err)
	}
	if err := output.Close(); err != nil {
		return fmt.Errorf("save replacement executable: %w", err)
	}
	return nil
}

func makeExecutable(path string) error {
	if runtime.GOOS == "windows" {
		return nil
	}
	if err := os.Chmod(path, 0o755); err != nil {
		return fmt.Errorf("make replacement executable: %w", err)
	}
	return nil
}

func scheduleWindowsReplacement(current, replacement, tempDir string) error {
	powershell, err := exec.LookPath("powershell.exe")
	if err != nil {
		return errors.New("powershell.exe is required to install a CLI update")
	}
	scriptPath := filepath.Join(tempDir, "replace.ps1")
	script := `param([int]$ParentPid, [string]$Source, [string]$Target, [string]$TempDir)
$ErrorActionPreference = 'Stop'
while (Get-Process -Id $ParentPid -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 100 }
for ($attempt = 0; $attempt -lt 50; $attempt++) {
    try {
        Copy-Item -LiteralPath $Source -Destination $Target -Force
        Remove-Item -LiteralPath $Source -Force
        Remove-Item -LiteralPath $TempDir -Recurse -Force
        exit 0
    } catch {
        Start-Sleep -Milliseconds 200
    }
}
exit 1
`
	if err := os.WriteFile(scriptPath, []byte(script), 0o600); err != nil {
		return fmt.Errorf("create Windows update helper: %w", err)
	}
	command := exec.Command(powershell,
		"-NoLogo", "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden",
		"-ExecutionPolicy", "Bypass", "-File", scriptPath,
		"-ParentPid", strconv.Itoa(os.Getpid()), "-Source", replacement, "-Target", current, "-TempDir", tempDir,
	)
	if err := command.Start(); err != nil {
		return fmt.Errorf("start Windows update helper: %w", err)
	}
	return nil
}
