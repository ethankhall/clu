#!/usr/bin/env sh

set -eux

date >> dates.txt
git add dates.txt
git commit -am 'Add a new line to EOF'