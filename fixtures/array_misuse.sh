#!/usr/bin/env bash

local -a files=(foo.txt bar.txt baz.txt)

# BT201: bare $files only gives first element
echo $files

# This is correct
echo "${files[@]}"
