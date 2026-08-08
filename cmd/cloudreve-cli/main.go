// cloudreve-cli is a JSON-first client intended for scripts and AI agents.
package main

import (
	"context"
	"fmt"
	"os"
	"os/signal"
)

var (
	version = "dev"
	commit  = "unknown"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()

	root := newRootCommand(os.Stdin, os.Stdout, os.Stderr)
	if err := root.ExecuteContext(ctx); err != nil {
		_, _ = fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
