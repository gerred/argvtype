#!/usr/bin/env bash
#@module bash>=5.2

#@sig deploy(cfg: Scalar[ExistingFile], manifests: Argv[ExistingFile]) -> Status[0] !may_exec
deploy() {
  #@bind $1 cfg
  #@bind $2.. manifests

  local cfg=$1
  shift
  local -a manifests=("$@")

  : "${cfg:?cfg required}"
  [[ -f $cfg ]] || return 1

  echo "Deploying with config: $cfg"
  echo "Manifests: ${manifests[@]}"
}

#@type KUBECONFIG: Scalar[ExistingFile]
