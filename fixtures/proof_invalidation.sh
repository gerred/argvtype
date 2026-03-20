#!/usr/bin/env bash
# Fixture exercising proof invalidation by effects (BT405/BT406/BT407)
# and custom proof sites (#@proves, command -v)

#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0] !may_exec
deploy() {
  #@bind $1 cfg
  echo "deploying $1"
}

#@proves $1 ExistingFile
validate_file() {
  [[ -f "$1" ]] || { echo "not found: $1" >&2; return 1; }
}

#@sig scan(dir: Scalar[ExistingDir]) -> Status[0]
scan() {
  #@bind $1 dir
  echo "scanning $1"
}

# --- Good: proof established and used without invalidation ---
cfg=/etc/app/config.yaml
[[ -f "$cfg" ]] || exit 1
echo "config validated"
deploy "$cfg"

# --- Bad: proof invalidated by rm ---
backup=/etc/app/backup.yaml
[[ -f "$backup" ]] || exit 1
rm -- old_backup.yaml
deploy "$backup"

# --- Bad: proof invalidated by cd ---
relcfg=./config.yaml
[[ -f "$relcfg" ]] || exit 1
cd /tmp || exit 1
deploy "$relcfg"

# --- Good: re-proof after invalidation ---
cfg2=/etc/app/other.yaml
[[ -f "$cfg2" ]] || exit 1
rm -- stale_file
[[ -f "$cfg2" ]] || exit 1
deploy "$cfg2"

# --- Good: custom proof function via #@proves ---
cfg3=/etc/app/third.yaml
validate_file "$cfg3"
deploy "$cfg3"

# --- Good: command -v as proof site ---
command -v jq || exit 1
echo "jq is available"
