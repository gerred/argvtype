#!/usr/bin/env bash
file="test.txt"
rm $file          # BT801 + BT202
rm "$file"        # OK
mv $file /tmp/    # BT801 + BT202
echo $file        # BT202 only (not destructive)
