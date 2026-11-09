# Sample regression matrix

34 CC0 RAW files from [raw.pixls.us](https://raw.pixls.us) (about 445 MB) that cover all ten supported extensions and several compression variants.

- `manifest.json`: file name, source URL, sha256, make, model, format, tune/holdout split, and `known_unsupported` reasons.
- `reference.json` and `reference/*.png`: per-file dimensions, mean RGB and a 64 px thumbnail of the default develop (about 180 KB in total).
- `cache/`: downloaded RAW files (gitignored).

Run it (downloads on first use and verifies every sha256):

```powershell
cargo test --release -p ccoonverter-core --test regression_matrix regression_matrix -- --ignored --nocapture --test-threads=1
```

The per-file table is printed and written to `target/regression/report.md`. A file with a `known_unsupported` reason may fail without failing the test; a regression on any other file does. After an intended change to the rendering, rewrite the references with:

```powershell
$env:CCOON_REGRESSION_BLESS = "1"
cargo test --release -p ccoonverter-core --test regression_matrix regression_matrix -- --ignored --nocapture --test-threads=1
```

Other reports from the same harness: `auto_exposure_holdout` (luminance against the camera JPEG on the tune and hold-out splits) and `metadata_report` (tags in real output files). In CI the workflow `.github/workflows/regression.yml` runs the matrix on a schedule, manually and on release tags, with the samples in `actions/cache`.
