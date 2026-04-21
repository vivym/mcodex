# non-CLI npm releases

Use the staging helper in the repo root to generate npm tarballs for non-CLI
release packages. The CLI is now distributed through the OSS installer, so this
helper stages only `codex-responses-api-proxy` and `codex-sdk`.

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.6.0 \
  --package codex-responses-api-proxy \
  --package codex-sdk
```

This downloads the native artifacts once, hydrates `vendor/` for each package,
and writes tarballs to `dist/npm/`.

If you need to invoke `build_npm_package.py` directly, run
`codex-cli/scripts/install_native_deps.py` first and pass `--vendor-src` pointing to the
directory that contains the populated `vendor/` tree.
