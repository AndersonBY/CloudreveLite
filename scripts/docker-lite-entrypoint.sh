#!/bin/sh
set -eu

seed_dir=/opt/cloudreve/statics
static_dir=/cloudreve/data/statics

if [ ! -e "${static_dir}" ]; then
  temp_dir="/cloudreve/data/.statics-seed.$$"
  trap 'rm -rf "${temp_dir:-}"' EXIT HUP INT TERM
  cp -a "${seed_dir}" "${temp_dir}"
  mv "${temp_dir}" "${static_dir}"
  trap - EXIT HUP INT TERM
fi

exec /usr/local/bin/cloudreve server --lite \
  --use-working-dir \
  --conf /cloudreve/data/conf.ini \
  "$@"
