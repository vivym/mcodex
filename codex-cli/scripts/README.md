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

This downloads native artifacts when a non-CLI package needs them and writes
tarballs to `dist/npm/`.

If you need to invoke `build_npm_package.py` directly, pass an explicit
non-CLI package:

```bash
codex-cli/scripts/build_npm_package.py \
  --package codex-responses-api-proxy \
  --release-version 0.6.0
```

Run `codex-cli/scripts/install_native_deps.py` first and pass `--vendor-src`
only for non-CLI packages that bundle native components.
