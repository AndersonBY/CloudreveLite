package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"runtime"
	"strconv"
	"strings"
	"time"

	"github.com/spf13/cobra"
)

const (
	requiresAPIAnnotation  = "cloudreve-cli/requires-api"
	requiresAuthAnnotation = "cloudreve-cli/requires-auth"
	secretLimit            = 1024
)

func newRootCommand(stdin io.Reader, stdout, stderr io.Writer) *cobra.Command {
	var (
		server    string
		token     string
		apiKey    string
		allowHTTP bool
		api       *client
	)

	root := &cobra.Command{
		Use:           "cloudreve-cli",
		Short:         "JSON-first CLI for CloudreveLite",
		SilenceUsage:  true,
		SilenceErrors: true,
		Args:          cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			return cmd.Help()
		},
	}
	root.SetIn(stdin)
	root.SetOut(stdout)
	root.SetErr(stderr)
	root.PersistentFlags().StringVar(&server, "server", os.Getenv("CLOUDREVE_SERVER"), "CloudreveLite URL (or CLOUDREVE_SERVER)")
	root.PersistentFlags().StringVar(&apiKey, "api-key", os.Getenv("CLOUDREVE_API_KEY"), "API key (or CLOUDREVE_API_KEY)")
	root.PersistentFlags().StringVar(&token, "token", os.Getenv("CLOUDREVE_TOKEN"), "legacy access token (or CLOUDREVE_TOKEN)")
	root.PersistentFlags().BoolVar(&allowHTTP, "allow-http", false, "allow credentials over plain HTTP")
	root.PersistentPreRunE = func(cmd *cobra.Command, _ []string) error {
		if cmd.Annotations[requiresAPIAnnotation] != "true" {
			return nil
		}
		if strings.TrimSpace(server) == "" {
			configured, err := configuredServer()
			if err != nil {
				return err
			}
			server = configured
		}
		if strings.TrimSpace(server) == "" {
			return errors.New("no CloudreveLite server is configured; run connect or pass --server")
		}
		validated, err := newClient(server, "", allowHTTP)
		if err != nil {
			return err
		}
		server = validated.base.String()
		if strings.TrimSpace(apiKey) != "" && strings.TrimSpace(token) != "" {
			return errors.New("API key and legacy access token cannot be used together")
		}
		credential := strings.TrimSpace(apiKey)
		if credential == "" {
			credential = strings.TrimSpace(token)
		}
		if cmd.Annotations[requiresAuthAnnotation] == "true" && credential == "" {
			var err error
			credential, err = storedAPIKey(server)
			if err != nil {
				return err
			}
			if credential == "" {
				return errors.New("no API key is available; run connect or set CLOUDREVE_API_KEY")
			}
		}
		api, err = newClient(server, credential, allowHTTP)
		return err
	}

	getClient := func() *client { return api }
	root.AddCommand(
		newVersionCommand(stdout),
		newUpdateCommand(stdout),
		newConnectCommand(getClient, stdout),
		newLoginCommand(getClient, stdin, stdout),
		newLogin2FACommand(getClient, stdin, stdout),
		newRefreshCommand(getClient, stdin, stdout),
		newMeCommand(getClient, stdout),
		newListCommand(getClient, stdout),
		newCreateCommand(getClient, stdout, "folder"),
		newCreateCommand(getClient, stdout, "file"),
		newRenameCommand(getClient, stdout),
		newMoveCommand(getClient, stdout),
		newDeleteCommand(getClient, stdout),
		newURLCommand(getClient, stdout),
		newUploadCommand(getClient, stdout),
		newDownloadCommand(getClient, stdout),
		newShareCommand(getClient, stdout),
	)
	return root
}

func markAPICommand(cmd *cobra.Command, tokenRequired bool) *cobra.Command {
	cmd.Annotations = map[string]string{requiresAPIAnnotation: "true"}
	if tokenRequired {
		cmd.Annotations[requiresAuthAnnotation] = "true"
	}
	return cmd
}

func newConnectCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	var (
		pairingCode string
		quiet       bool
	)
	cmd := &cobra.Command{
		Use:   "connect",
		Short: "Connect this device with a one-time pairing code",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			if strings.TrimSpace(pairingCode) == "" {
				return errors.New("--pairing-code is required")
			}
			var result struct {
				ID        string    `json:"id"`
				Name      string    `json:"name"`
				Key       string    `json:"key"`
				ExpiresAt time.Time `json:"expires_at"`
			}
			raw, err := getClient().request(cmd.Context(), http.MethodPost, "/session/api-key/exchange", map[string]string{
				"code": pairingCode,
			})
			if err != nil {
				return err
			}
			if err := json.Unmarshal(raw, &result); err != nil {
				return fmt.Errorf("decode pairing response: %w", err)
			}
			if !strings.HasPrefix(result.Key, "crl_") {
				return errors.New("server returned an invalid API key")
			}
			server := getClient().base.String()
			if err := saveConnection(server, result.Key); err != nil {
				return err
			}
			if !quiet {
				if err := printConnectGuide(cmd.ErrOrStderr(), server, result.ExpiresAt); err != nil {
					return err
				}
			}
			return printValue(stdout, map[string]any{
				"connected":        true,
				"server":           server,
				"key_id":           result.ID,
				"name":             result.Name,
				"expires_at":       result.ExpiresAt,
				"credential_store": "system",
			})
		},
	}
	cmd.Flags().StringVar(&pairingCode, "pairing-code", "", "short-lived pairing code from the CloudreveLite web app")
	cmd.Flags().BoolVar(&quiet, "quiet", false, "suppress the post-connect usage guide")
	return markAPICommand(cmd, false)
}

func printConnectGuide(writer io.Writer, server string, expiresAt time.Time) error {
	_, err := fmt.Fprintf(writer, `
Connected to CloudreveLite.
Your API key was saved in the system credential manager and will not be printed.

Start here:
  cloudreve-cli me
  cloudreve-cli list cloudreve://my/

Upload and download:
  cloudreve-cli upload ./file.txt cloudreve://my/
  cloudreve-cli download cloudreve://my/file.txt ./file.txt

More commands:
  cloudreve-cli --help
  cloudreve-cli <command> --help

Server: %s
API key expires: %s

`, server, expiresAt.Format(time.RFC3339))
	return err
}

func newVersionCommand(stdout io.Writer) *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Show CLI build information",
		Args:  cobra.NoArgs,
		RunE: func(_ *cobra.Command, _ []string) error {
			return printValue(stdout, map[string]string{
				"version": version,
				"commit":  commit,
				"go":      runtime.Version(),
				"os":      runtime.GOOS,
				"arch":    runtime.GOARCH,
			})
		},
	}
}

func newLoginCommand(getClient func() *client, stdin io.Reader, stdout io.Writer) *cobra.Command {
	var passwordStdin bool
	cmd := &cobra.Command{
		Use:   "login EMAIL",
		Short: "Issue an access/refresh token pair",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			password, err := readSecret(stdin, "CLOUDREVE_PASSWORD", passwordStdin, "password")
			if err != nil {
				return err
			}
			result, err := getClient().doAPI(cmd.Context(), http.MethodPost, "/session/token", map[string]string{
				"email":    args[0],
				"password": password,
			})
			if err != nil {
				return err
			}
			if result.Code == 203 {
				var sessionID string
				if err := json.Unmarshal(result.Data, &sessionID); err != nil {
					return fmt.Errorf("decode two-factor session: %w", err)
				}
				if otp := strings.TrimSpace(os.Getenv("CLOUDREVE_OTP")); otp != "" {
					return complete2FA(cmd, getClient(), sessionID, otp, stdout)
				}
				return printValue(stdout, map[string]any{
					"two_factor_required": true,
					"session_id":          sessionID,
				})
			}
			if result.Code != 0 {
				return errorFromEnvelope(result)
			}
			return printJSON(stdout, result.Data)
		},
	}
	cmd.Flags().BoolVar(&passwordStdin, "password-stdin", false, "read the password from stdin instead of CLOUDREVE_PASSWORD")
	return markAPICommand(cmd, false)
}

func newLogin2FACommand(getClient func() *client, stdin io.Reader, stdout io.Writer) *cobra.Command {
	var otpStdin bool
	cmd := &cobra.Command{
		Use:   "login-2fa SESSION_ID",
		Short: "Complete a login that requires TOTP",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			otp, err := readSecret(stdin, "CLOUDREVE_OTP", otpStdin, "OTP")
			if err != nil {
				return err
			}
			return complete2FA(cmd, getClient(), args[0], otp, stdout)
		},
	}
	cmd.Flags().BoolVar(&otpStdin, "otp-stdin", false, "read the OTP from stdin instead of CLOUDREVE_OTP")
	return markAPICommand(cmd, false)
}

func complete2FA(cmd *cobra.Command, api *client, sessionID, otp string, stdout io.Writer) error {
	raw, err := api.request(cmd.Context(), http.MethodPost, "/session/token/2fa", map[string]string{
		"session_id": sessionID,
		"otp":        otp,
	})
	if err != nil {
		return err
	}
	return printJSON(stdout, raw)
}

func newRefreshCommand(getClient func() *client, stdin io.Reader, stdout io.Writer) *cobra.Command {
	var refreshStdin bool
	cmd := &cobra.Command{
		Use:   "refresh",
		Short: "Refresh an access/refresh token pair",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			refreshToken, err := readSecret(stdin, "CLOUDREVE_REFRESH_TOKEN", refreshStdin, "refresh token")
			if err != nil {
				return err
			}
			raw, err := getClient().request(cmd.Context(), http.MethodPost, "/session/token/refresh", map[string]string{
				"refresh_token": refreshToken,
			})
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	cmd.Flags().BoolVar(&refreshStdin, "refresh-token-stdin", false, "read the refresh token from stdin instead of CLOUDREVE_REFRESH_TOKEN")
	return markAPICommand(cmd, false)
}

func newMeCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "me",
		Short: "Show the authenticated user",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			raw, err := getClient().request(cmd.Context(), http.MethodGet, "/user/me", nil)
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	return markAPICommand(cmd, true)
}

func newListCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	var pageSize int
	var pageToken string
	cmd := &cobra.Command{
		Use:     "list URI",
		Aliases: []string{"ls"},
		Short:   "List a directory",
		Args:    cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			query := url.Values{"uri": {args[0]}}
			if pageSize > 0 {
				query.Set("page_size", strconv.Itoa(pageSize))
			}
			if pageToken != "" {
				query.Set("next_page_token", pageToken)
			}
			raw, err := getClient().request(cmd.Context(), http.MethodGet, "/file?"+query.Encode(), nil)
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	cmd.Flags().IntVar(&pageSize, "page-size", 0, "maximum items to return")
	cmd.Flags().StringVar(&pageToken, "page-token", "", "pagination token from a previous response")
	return markAPICommand(cmd, true)
}

func newCreateCommand(getClient func() *client, stdout io.Writer, kind string) *cobra.Command {
	cmd := &cobra.Command{
		Use:   kind + " URI",
		Short: "Create a " + kind,
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			raw, err := getClient().request(cmd.Context(), http.MethodPost, "/file/create", map[string]any{
				"uri":             args[0],
				"type":            kind,
				"err_on_conflict": true,
			})
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	return markAPICommand(cmd, true)
}

func newRenameCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "rename URI NEW_NAME",
		Short: "Rename a file or folder",
		Args:  cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			raw, err := getClient().request(cmd.Context(), http.MethodPost, "/file/rename", map[string]string{
				"uri":      args[0],
				"new_name": args[1],
			})
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	return markAPICommand(cmd, true)
}

func newMoveCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	var copyFile bool
	cmd := &cobra.Command{
		Use:   "move URI DESTINATION",
		Short: "Move or copy an item",
		Args:  cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			raw, err := getClient().request(cmd.Context(), http.MethodPost, "/file/move", map[string]any{
				"uris": []string{args[0]},
				"dst":  args[1],
				"copy": copyFile,
			})
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	cmd.Flags().BoolVar(&copyFile, "copy", false, "copy instead of move")
	return markAPICommand(cmd, true)
}

func newDeleteCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	cmd := &cobra.Command{
		Use:     "delete URI",
		Aliases: []string{"rm"},
		Short:   "Move an item to trash",
		Args:    cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			raw, err := getClient().request(cmd.Context(), http.MethodDelete, "/file", map[string]any{"uris": []string{args[0]}})
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	return markAPICommand(cmd, true)
}

func newURLCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	var download bool
	cmd := &cobra.Command{
		Use:   "url URI",
		Short: "Get a temporary preview/download URL",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			raw, err := getClient().request(cmd.Context(), http.MethodPost, "/file/url", map[string]any{
				"uris":     []string{args[0]},
				"download": download,
			})
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	cmd.Flags().BoolVar(&download, "download", false, "request a download URL")
	return markAPICommand(cmd, true)
}

func newUploadCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	var policyID string
	var mimeType string
	var overwrite bool
	cmd := &cobra.Command{
		Use:   "upload LOCAL_FILE DESTINATION_URI",
		Short: "Upload a local file directly to OSS or S3",
		Args:  cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			result, err := uploadFile(cmd.Context(), getClient(), args[0], args[1], uploadOptions{
				PolicyID:  policyID,
				MimeType:  mimeType,
				Overwrite: overwrite,
			})
			if err != nil {
				return err
			}
			return printJSON(stdout, result)
		},
	}
	cmd.Flags().StringVar(&policyID, "policy", "", "storage policy ID (defaults to the user's policy)")
	cmd.Flags().StringVar(&mimeType, "mime-type", "", "override the detected MIME type")
	cmd.Flags().BoolVar(&overwrite, "overwrite", false, "upload a new version when the destination exists")
	return markAPICommand(cmd, true)
}

func newDownloadCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	var force bool
	cmd := &cobra.Command{
		Use:   "download URI [LOCAL_FILE]",
		Short: "Download a file to local disk",
		Args:  cobra.RangeArgs(1, 2),
		RunE: func(cmd *cobra.Command, args []string) error {
			localPath := ""
			if len(args) == 2 {
				localPath = args[1]
			}
			result, err := downloadFile(cmd.Context(), getClient(), args[0], localPath, force)
			if err != nil {
				return err
			}
			return printValue(stdout, result)
		},
	}
	cmd.Flags().BoolVar(&force, "force", false, "replace an existing local file")
	return markAPICommand(cmd, true)
}

func newShareCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	share := &cobra.Command{Use: "share", Short: "Create and manage temporary shares", Args: cobra.NoArgs, RunE: func(cmd *cobra.Command, _ []string) error {
		return cmd.Help()
	}}
	share.AddCommand(
		newShareCreateCommand(getClient, stdout),
		newShareListCommand(getClient, stdout),
		newShareDeleteCommand(getClient, stdout),
	)
	return share
}

func newShareCreateCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	var days int
	var downloads int
	var private bool
	cmd := &cobra.Command{
		Use:   "create URI",
		Short: "Create an expiring share link",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			if days < 1 || days > 30 {
				return errors.New("--days must be between 1 and 30")
			}
			if downloads < 1 || downloads > 100 {
				return errors.New("--downloads must be between 1 and 100")
			}
			password := strings.TrimSpace(os.Getenv("CLOUDREVE_SHARE_PASSWORD"))
			if password != "" {
				private = true
			}
			raw, err := getClient().request(cmd.Context(), http.MethodPut, "/share", map[string]any{
				"uri":        args[0],
				"is_private": private,
				"password":   password,
				"downloads":  downloads,
				"expire":     days * 24 * int(time.Hour/time.Second),
			})
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	cmd.Flags().IntVar(&days, "days", 7, "share lifetime in days (1-30)")
	cmd.Flags().IntVar(&downloads, "downloads", 10, "maximum downloads (1-100)")
	cmd.Flags().BoolVar(&private, "private", false, "password-protect the share using a generated password")
	return markAPICommand(cmd, true)
}

func newShareListCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	var pageSize int
	var pageToken string
	cmd := &cobra.Command{
		Use:   "list",
		Short: "List shares owned by the authenticated user",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			if pageSize < 10 || pageSize > 100 {
				return errors.New("--page-size must be between 10 and 100")
			}
			query := url.Values{"page_size": {strconv.Itoa(pageSize)}}
			if pageToken != "" {
				query.Set("next_page_token", pageToken)
			}
			raw, err := getClient().request(cmd.Context(), http.MethodGet, "/share?"+query.Encode(), nil)
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	cmd.Flags().IntVar(&pageSize, "page-size", 50, "maximum shares to return (10-100)")
	cmd.Flags().StringVar(&pageToken, "page-token", "", "pagination token from a previous response")
	return markAPICommand(cmd, true)
}

func newShareDeleteCommand(getClient func() *client, stdout io.Writer) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "delete SHARE_ID",
		Short: "Delete a share",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			raw, err := getClient().request(cmd.Context(), http.MethodDelete, "/share/"+url.PathEscape(args[0]), nil)
			if err != nil {
				return err
			}
			return printJSON(stdout, raw)
		},
	}
	return markAPICommand(cmd, true)
}

func readSecret(stdin io.Reader, envName string, fromStdin bool, label string) (string, error) {
	fromEnvironment := os.Getenv(envName)
	if fromStdin && fromEnvironment != "" {
		return "", fmt.Errorf("%s is set and stdin was also requested; choose one secret source", envName)
	}
	secret := fromEnvironment
	if fromStdin {
		raw, err := io.ReadAll(io.LimitReader(stdin, secretLimit+1))
		if err != nil {
			return "", fmt.Errorf("read %s from stdin: %w", label, err)
		}
		if len(raw) > secretLimit {
			return "", fmt.Errorf("%s exceeds %d bytes", label, secretLimit)
		}
		secret = strings.TrimRight(string(raw), "\r\n")
	}
	if secret == "" {
		return "", fmt.Errorf("%s is required; set %s or use the stdin flag", label, envName)
	}
	if strings.ContainsAny(secret, "\r\n") {
		return "", fmt.Errorf("%s must be a single line", label)
	}
	return secret, nil
}

func printJSON(writer io.Writer, raw []byte) error {
	if len(raw) == 0 {
		raw = []byte("null")
	}
	var output bytes.Buffer
	if err := json.Indent(&output, raw, "", "  "); err != nil {
		return fmt.Errorf("format JSON response: %w", err)
	}
	_, err := fmt.Fprintln(writer, output.String())
	return err
}

func printValue(writer io.Writer, value any) error {
	raw, err := json.Marshal(value)
	if err != nil {
		return err
	}
	return printJSON(writer, raw)
}
