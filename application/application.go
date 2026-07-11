package application

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"time"

	"github.com/cloudreve/Cloudreve/v4/application/constants"
	"github.com/cloudreve/Cloudreve/v4/application/dependency"
	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/cloudreve/Cloudreve/v4/pkg/cache"
	"github.com/cloudreve/Cloudreve/v4/pkg/conf"
	"github.com/cloudreve/Cloudreve/v4/pkg/crontab"
	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/cloudreve/Cloudreve/v4/pkg/setting"
	"github.com/cloudreve/Cloudreve/v4/pkg/util"
	"github.com/cloudreve/Cloudreve/v4/routers"
	"github.com/gin-gonic/gin"
)

type Server interface {
	// Start starts the Cloudreve server.
	Start() error
	PrintBanner()
	Close()
}

// NewServer constructs a new Cloudreve server instance with given dependency.
func NewServer(dep dependency.Dep) Server {
	return &server{
		dep:    dep,
		logger: dep.Logger(),
		config: dep.ConfigProvider(),
	}
}

type server struct {
	dep      dependency.Dep
	logger   logging.Logger
	dbClient *ent.Client
	config   conf.ConfigProvider
	server   *http.Server
	kv       cache.Driver
}

func (s *server) PrintBanner() {
	fmt.Print(`
   ___ _                 _                    
  / __\ | ___  _   _  __| |_ __ _____   _____ 
 / /  | |/ _ \| | | |/ _  | '__/ _ \ \ / / _ \	
/ /___| | (_) | |_| | (_| | | |  __/\ V /  __/
\____/|_|\___/ \__,_|\__,_|_|  \___| \_/ \___|

   V` + constants.BackendVersion + `  Commit #` + constants.LastCommit + `  Pro=` + constants.IsPro + `
================================================

`)
}

func (s *server) Start() error {
	// Debug 关闭时，切换为生产模式
	if !s.config.System().Debug {
		gin.SetMode(gin.ReleaseMode)
	}

	s.kv = s.dep.KV()
	// delete all cached settings
	_ = s.kv.Delete(setting.KvSettingPrefix)
	if memKv, ok := s.kv.(*cache.MemoStore); ok {
		memKv.GarbageCollect(s.logger)
	}

	if s.config.System().Mode != conf.MasterMode {
		return fmt.Errorf("CloudreveLite only supports master mode")
	}

	s.dbClient = s.dep.DBClient()
	s.dep.LockSystem()
	s.dep.EntityRecycleQueue(context.Background()).Start()

	cron, err := crontab.NewCron(context.Background(), s.dep)
	if err != nil {
		return err
	}
	cron.Start()

	api := routers.InitRouter(s.dep)
	api.TrustedPlatform = s.config.System().ProxyHeader
	s.server = &http.Server{Handler: api}

	// 如果启用了SSL
	if s.config.SSL().CertPath != "" {
		s.logger.Info("Listening to %q", s.config.SSL().Listen)
		s.server.Addr = s.config.SSL().Listen
		if err := s.server.ListenAndServeTLS(s.config.SSL().CertPath, s.config.SSL().KeyPath); err != nil && !errors.Is(err, http.ErrServerClosed) {
			return fmt.Errorf("failed to listen to %q: %w", s.config.SSL().Listen, err)
		}

		return nil
	}

	// 如果启用了Unix
	if s.config.Unix().Listen != "" {
		// delete socket file before listening
		if _, err := os.Stat(s.config.Unix().Listen); err == nil {
			if err = os.Remove(s.config.Unix().Listen); err != nil {
				return fmt.Errorf("failed to delete socket file %q: %w", s.config.Unix().Listen, err)
			}
		}

		s.logger.Info("Listening to %q", s.config.Unix().Listen)
		if err := s.runUnix(s.server); err != nil && !errors.Is(err, http.ErrServerClosed) {
			return fmt.Errorf("failed to listen to %q: %w", s.config.Unix().Listen, err)
		}

		return nil
	}

	s.logger.Info("Listening to %q", s.config.System().Listen)
	s.server.Addr = s.config.System().Listen
	if err := s.server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		return fmt.Errorf("failed to listen to %q: %w", s.config.System().Listen, err)
	}
	return nil
}

func (s *server) Close() {
	if s.dbClient != nil {
		s.logger.Info("Shutting down database connection...")
		if err := s.dbClient.Close(); err != nil {
			s.logger.Error("Failed to close database connection: %s", err)
		}
	}

	ctx := context.Background()
	if conf.SystemConfig.GracePeriod != 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, time.Duration(s.config.System().GracePeriod)*time.Second)
		defer cancel()
	}

	s.dep.EventHub().Close()

	// Shutdown http server
	if s.server != nil {
		err := s.server.Shutdown(ctx)
		if err != nil {
			s.logger.Error("Failed to shutdown server: %s", err)
		}
	}

	if s.kv != nil {
		if err := s.kv.Persist(util.DataPath(cache.DefaultCacheFile)); err != nil {
			s.logger.Warning("Failed to persist cache: %s", err)
		}
	}

	if err := s.dep.Shutdown(ctx); err != nil {
		s.logger.Warning("Failed to shutdown dependency manager: %s", err)
	}
}

func (s *server) runUnix(server *http.Server) error {
	listener, err := net.Listen("unix", s.config.Unix().Listen)
	if err != nil {
		return err
	}

	defer listener.Close()
	defer os.Remove(s.config.Unix().Listen)

	if conf.UnixConfig.Perm > 0 {
		err = os.Chmod(conf.UnixConfig.Listen, os.FileMode(s.config.Unix().Perm))
		if err != nil {
			s.logger.Warning(
				"Failed to set permission to %q for socket file %q: %s",
				s.config.Unix().Perm,
				s.config.Unix().Listen,
				err,
			)
		}
	}

	return server.Serve(listener)
}
