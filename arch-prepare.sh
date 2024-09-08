#!/bin/bash

set -euo pipefail

cargo b --release
cp ./target/release/dircacher ./arch-pkg/dircacher
cp ./systemd/dircacher.service ./arch-pkg/dircacher.service
