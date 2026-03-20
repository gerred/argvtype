#!/usr/bin/env bash
cd /tmp
rm -rf build/        # BT802: cd followed by rm without error check

cd /var && rm old    # OK: guarded with &&
cd /opt || exit 1    # OK: guarded with || exit
