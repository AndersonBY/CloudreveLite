# CloudreveLite CLI

`cloudreve-cli` is a non-interactive, JSON-first client for AI agents, shell scripts, and CI. It supports revocable API keys, file operations, direct OSS/S3 uploads, downloads, and temporary share management.

## Download

CLI releases use independent `cli-vX.Y.Z` tags; the current CLI release line is
`cli-v0.1.2`. Pushing one of these tags runs tests and builds all six platform
archives. Tray releases do not replace the repository's `latest` download target,
so the CLI install commands continue to find their archives and checksums.

Each CLI release publishes Linux, macOS, and Windows archives for amd64 and arm64. Verify the downloaded archive against the `SHA256SUMS` file in the same release before running it.

The CLI can also be built from source:

```bash
go build -o cloudreve-cli ./cmd/cloudreve-cli
```

## Authentication

Open **Settings -> Password and security -> Install and connect CLI** in your CloudreveLite site. Select the operating system, generate the command, and run it in PowerShell or a POSIX shell. The command:

1. Downloads the latest CLI from this repository's GitHub Release.
2. Verifies the archive against the published `SHA256SUMS`.
3. Exchanges a 10-minute, single-use pairing code for an API key.
4. Stores the API key in Windows Credential Manager, macOS Keychain, or Linux Secret Service.

The pairing command contains no password or long-lived API key. After a successful connection, commands automatically use the saved server and credential:

```bash
cloudreve-cli me
cloudreve-cli list cloudreve://my/
```

The connection result remains JSON on stdout for agents and scripts. For an interactive install, the CLI also prints a short getting-started guide to stderr with the first `me`, `list`, `upload`, and `download` commands. Use `connect --quiet` to suppress that guide.

For CI or an external secret manager, skip local storage and set both values explicitly:

```bash
export CLOUDREVE_SERVER=https://cloud.example.com
export CLOUDREVE_API_KEY='...'
cloudreve-cli me
```

API keys can read basic account information and manage the owning account's files and shares. They cannot change security settings, create more keys, or use administrator APIs. Revoke a key from the web security page to stop it immediately.

Password/TOTP login and refresh-token commands remain available for compatibility. They never accept passwords, OTP values, or refresh tokens as positional arguments; provide those values through their documented environment variables or stdin flags.

## Agent workflow

Cloudreve URIs use forms such as `cloudreve://my/` and `cloudreve://my/results/report.pdf`.

```bash
cloudreve-cli me
cloudreve-cli list cloudreve://my/
cloudreve-cli folder cloudreve://my/results

cloudreve-cli upload ./report.pdf cloudreve://my/results/
cloudreve-cli share create cloudreve://my/results/report.pdf
cloudreve-cli share list

cloudreve-cli download cloudreve://my/results/report.pdf ./report-copy.pdf
cloudreve-cli url --download cloudreve://my/results/report.pdf

cloudreve-cli rename cloudreve://my/results/report.pdf final-report.pdf
cloudreve-cli move cloudreve://my/results/final-report.pdf cloudreve://my/
cloudreve-cli delete cloudreve://my/final-report.pdf
```

When an upload destination ends with `/`, the local filename is appended. Otherwise, the destination is treated as the complete file URI. `--overwrite` requests a new version for an existing destination. Upload data goes directly to signed OSS/S3 URLs and never receives the Cloudreve bearer token. Relayed uploads and storage policies other than OSS/S3 are rejected by the Lite CLI.

The CLI reuses its transfer client and connection pool across parts and completion
requests. A regression test verifies a single storage connection for both OSS and
S3 multipart uploads. Parts remain sequential, so response waits and retries can
still produce gaps in network activity.

Shares default to 7 days and 10 downloads. The CLI enforces the Lite limits of 1-30 days and 1-100 downloads:

```bash
cloudreve-cli share create --days 14 --downloads 25 cloudreve://my/results/report.pdf
cloudreve-cli share delete SHARE_ID
```

Pass `--private` to let the server generate a share password. To choose an alphanumeric password, set `CLOUDREVE_SHARE_PASSWORD`; it is never accepted as a command-line argument.

All successful command results are written as JSON to stdout. Errors are written to stderr and produce a nonzero exit code. Run `cloudreve-cli --help` or `cloudreve-cli <command> --help` for the complete local command reference.

Plain HTTP is accepted automatically only for loopback servers. For a trusted non-loopback development instance, `--allow-http` is required; HTTPS remains the recommended configuration.
