#!/usr/bin/env bash

# No BT302: guard with :? then use
safe_deploy() {
  local cfg
  : "${cfg:?cfg is required}"
  echo "$cfg"
}

# No BT302: use default operator
safe_default() {
  local name
  echo "${name:-anonymous}"
}

# No BT302: assign before use
safe_assign() {
  local x
  x=hello
  echo "$x"
}
