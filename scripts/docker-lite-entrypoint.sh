#!/bin/sh
set -eu

exec /usr/local/bin/cloudreve server \
  --use-working-dir \
  --conf /cloudreve/data/conf.ini \
  "$@"
