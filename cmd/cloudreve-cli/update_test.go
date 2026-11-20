package main

import "testing"

func TestSelectCLIUpdateUsesCLIReleaseMetadata(t *testing.T) {
	release := func(tag string, cliVersion string) githubRelease {
		return githubRelease{
			TagName: tag,
			Assets: []githubAsset{
				{Name: "cloudreve-cli-version-v" + cliVersion + ".txt"},
				{
					Name:               "cloudreve-cli_windows_amd64.zip",
					BrowserDownloadURL: githubDownloadsURL + "/" + tag + "/cloudreve-cli_windows_amd64.zip",
					Size:               123,
				},
			},
		}
	}

	update, err := selectCLIUpdate([]githubRelease{
		release("tray-v0.3.4", "0.1.4"),
		release("cli-v0.1.3", "0.1.3"),
		release("cli-v0.1.2", "0.1.2"),
	}, [3]uint64{0, 1, 2}, "windows", "amd64")
	if err != nil {
		t.Fatal(err)
	}
	if update == nil || update.ReleaseTag != "tray-v0.3.4" || update.Version != "cli-v0.1.4" {
		t.Fatalf("selected update = %#v", update)
	}
}

func TestChecksumForAsset(t *testing.T) {
	want := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
	for _, text := range []string{
		want + "  cloudreve-cli_windows_amd64.zip\n",
		want + " *cloudreve-cli_windows_amd64.zip\n",
	} {
		got, err := checksumForAsset(text, "cloudreve-cli_windows_amd64.zip")
		if err != nil || got != want {
			t.Fatalf("checksum = %q, err = %v", got, err)
		}
	}
	if _, err := checksumForAsset("bad  cloudreve-cli_windows_amd64.zip\n", "cloudreve-cli_windows_amd64.zip"); err == nil {
		t.Fatal("invalid checksum was accepted")
	}
}
