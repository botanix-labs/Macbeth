#!/usr/bin/env bash

[[ $DEBUG = true ]] && set -x
set -euo pipefail

tilt up -f Tiltfile
