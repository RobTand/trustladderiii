#!/bin/bash
set -euo pipefail
: "${PQ_BASE_IMAGE:?set a digest-pinned python:3.12-slim-bookworm reference}"
: "${PQ_BASE_CONFIG_DIGEST:?set the corresponding config digest}"
: "${PQ_DEBIAN_SNAPSHOT:?set the reviewed Debian snapshot timestamp}"
exec /usr/bin/python3 -B harness/tools/flux_prismaquant_image.py \
    --base "$PQ_BASE_IMAGE" --base-config-digest "$PQ_BASE_CONFIG_DIGEST" \
    --debian-snapshot "$PQ_DEBIAN_SNAPSHOT"
