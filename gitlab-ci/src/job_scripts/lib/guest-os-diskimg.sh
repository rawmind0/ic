#!/usr/bin/env bash
#
# Script for gutest-os-diskimg CI job
# Inputs:
#  - artifacts/release directory with artifacts
#

set -euo pipefail

BUILD_OUT=${1:-"build-out/disk-img"}
BUILD_TMP=${2:-"build-tmp"}
UPLOAD_TARGET=${3:-"guest-os/disk-img"}
VERSION=${4:-$(git rev-parse --verify HEAD)}
CDPRNET=${5:-"cdpr05"}

ROOT_DIR=$(git rev-parse --show-toplevel)
ls -lah /var/run/docker.sock
groups

cd "$ROOT_DIR" || exit 1
for f in replica orchestrator canister_sandbox sandbox_launcher vsock_agent state-tool ic-consensus-pool-util ic-crypto-csp ic-regedit ic-recovery ic-btc-adapter ic-canister-http-adapter; do
    gunzip -c -d artifacts/release/$f.gz >artifacts/release/$f
done

# if we are building the malicious image, use malicious replica version
if [[ "${BUILD_EXTRA_SUFFIX}" =~ "malicious" ]]; then
    gunzip -c -d artifacts/release-malicious/replica.gz >artifacts/release/replica
    chmod +x artifacts/release/replica
fi

cd "$ROOT_DIR"/ic-os/guestos || exit 1
mkdir -p "$BUILD_OUT" "$BUILD_TMP"
echo "$VERSION" >"${BUILD_TMP}/version.txt"

if [ -z "$CI_JOB_ID" ]; then
    # shellcheck disable=SC2086  # Expanding BUILD_EXTRA_ARGS into multiple parameters
    ./scripts/build-disk-image.sh -o "${BUILD_TMP}/disk.img" -v "$VERSION" -x ../../artifacts/release/ $BUILD_EXTRA_ARGS
    tar --sort=name --owner=root:0 --group=root:0 --mtime='UTC 2020-01-01' --sparse \
        -cvzf "${BUILD_OUT}/disk-img.tar.gz" -C "$BUILD_TMP" disk.img version.txt
    tar --sort=name --owner=root:0 --group=root:0 --mtime='UTC 2020-01-01' --sparse \
        -cvf "${BUILD_OUT}/disk-img.tar.zst" --use-compress-program="zstd --threads=0 -10" \
        -C "$BUILD_TMP" disk.img version.txt
    ls -lah "$BUILD_TMP"
else
    # shellcheck disable=SC2086  # Expanding BUILD_EXTRA_ARGS into multiple parameters
    buildevents cmd "${ROOT_PIPELINE_ID}" "${CI_JOB_ID}" build-disk-img -- \
        ./scripts/build-disk-image.sh -o "${BUILD_TMP}/disk.img" -v "$VERSION" -x ../../artifacts/release/ $BUILD_EXTRA_ARGS
    buildevents cmd "$ROOT_PIPELINE_ID" "$CI_JOB_ID" tar-build-out -- \
        tar --sort=name --owner=root:0 --group=root:0 --mtime='UTC 2020-01-01' --sparse \
        -cvzf "${BUILD_OUT}/disk-img.tar.gz" -C "$BUILD_TMP" disk.img version.txt
    buildevents cmd "$ROOT_PIPELINE_ID" "$CI_JOB_ID" tar-build-out -- \
        tar --sort=name --owner=root:0 --group=root:0 --mtime='UTC 2020-01-01' --sparse \
        -cvf "${BUILD_OUT}/disk-img.tar.zst" --use-compress-program="zstd --threads=0 -10" \
        -C "$BUILD_TMP" disk.img version.txt
    ls -lah "$BUILD_TMP"

    "$ROOT_DIR"/gitlab-ci/src/artifacts/openssl-sign.sh "$BUILD_OUT"
fi
