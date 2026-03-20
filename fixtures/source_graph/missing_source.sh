#!/usr/bin/env bash
# Sources a file that doesn't exist — should produce BT701

source nonexistent_lib.sh

echo "this should warn"
