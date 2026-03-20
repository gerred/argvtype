#!/usr/bin/env bash
file="test.txt"
rm $file          # BT202
rm "$file"        # OK
x=$file           # OK (assignment)
for f in $file; do echo "$f"; done  # OK (for items)
[[ -f $file ]]    # OK (test command)
echo $?           # OK (special var)
cp $file /tmp/    # BT202 on $file
