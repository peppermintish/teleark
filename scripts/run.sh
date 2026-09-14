#!/bin/bash
# Build and launch using the repository's private local configuration.
exec "$(dirname "$0")/build-local.sh" --run "$@"
