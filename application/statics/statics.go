package statics

import (
	"encoding/json"
	"fmt"
	"io"
	"os"

	"github.com/cloudreve/Cloudreve/v4/application/constants"
	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/cloudreve/Cloudreve/v4/pkg/util"
	"github.com/gin-contrib/static"
)

const (
	StaticFolder  = "statics"
	StaticPathEnv = "CLOUDREVE_STATIC_DIR"
)

type version struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// NewServerStaticFS serves the frontend from disk without embedding it in the Go binary.
func NewServerStaticFS(l logging.Logger, isPro bool) (static.ServeFileSystem, error) {
	staticPath := os.Getenv(StaticPathEnv)
	if staticPath == "" {
		staticPath = util.DataPath(StaticFolder)
	}
	if !util.Exists(staticPath) {
		return nil, fmt.Errorf("static resource folder %q does not exist", staticPath)
	}

	l.Info("Serving static files from %q.", staticPath)
	staticFS := static.LocalFile(staticPath, false)
	f, err := staticFS.Open("version.json")
	if err != nil {
		l.Warning("Missing version identifier file in static resources.")
		return staticFS, nil
	}
	defer f.Close()

	b, err := io.ReadAll(f)
	if err != nil {
		l.Warning("Failed to read version identifier file in static resources.")
		return staticFS, nil
	}

	var v version
	if err := json.Unmarshal(b, &v); err != nil {
		l.Warning("Failed to parse version identifier file in static resources: %s", err)
		return staticFS, nil
	}

	staticName := "cloudreve-frontend"
	if isPro {
		staticName += "-pro"
	}
	if v.Name != staticName {
		l.Error("Static resource name mismatch [Current: %s, Desired: %s].", v.Name, staticName)
	}
	if v.Version != constants.BackendVersion {
		l.Error("Static resource version mismatch [Current: %s, Desired: %s].", v.Version, constants.BackendVersion)
	}

	return staticFS, nil
}
