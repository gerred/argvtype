#!/usr/bin/env bash

name="world"
echo "Hello, $name"

if [[ -n $name ]]; then
  echo "Name is set"
fi
