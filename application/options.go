package application

type startupProfile struct {
	oneDriveCredentials bool
	emailQueue          bool
	mediaMetaQueue      bool
	entityRecycleQueue  bool
	ioIntenseQueue      bool
	remoteDownloadQueue bool
	thumbQueue          bool
}

func defaultStartupProfile() startupProfile {
	return startupProfile{
		oneDriveCredentials: true,
		emailQueue:          true,
		mediaMetaQueue:      true,
		entityRecycleQueue:  true,
		ioIntenseQueue:      true,
		remoteDownloadQueue: true,
		thumbQueue:          true,
	}
}

func liteStartupProfile() startupProfile {
	return startupProfile{entityRecycleQueue: true}
}

type serverOptions struct {
	startupProfile startupProfile
}

// ServerOption configures server startup behavior.
type ServerOption interface {
	apply(*serverOptions)
}

type serverOptionFunc func(*serverOptions)

func (f serverOptionFunc) apply(options *serverOptions) {
	f(options)
}

func newServerOptions(opts ...ServerOption) serverOptions {
	options := serverOptions{startupProfile: defaultStartupProfile()}
	for _, opt := range opts {
		opt.apply(&options)
	}

	return options
}

// WithLiteMode skips startup work that is unnecessary for the Lite deployment profile.
func WithLiteMode() ServerOption {
	return serverOptionFunc(func(options *serverOptions) {
		options.startupProfile = liteStartupProfile()
	})
}
