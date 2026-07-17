

I've now thoroughly inspected the README against all referenced project files, CLI output, and source code. Here are my findings:

---

## Review of `README.md` — `/workspace/trcc-display/README.md`

### Pros

- **Purpose is immediately obvious**: The first line + short paragraph clearly state what the tool does (drives Thermalright cooler 7-segment displays from live metrics). The hardware USB ID is called out upfront.
- **Well-structured sections**: Features, architecture diagram, quick start, configuration table, REST API table, CLI, device profiles, development, CI, deployment — logical progression from "what is this" to "how do I run it" to "how do I contribute."
- **Accuracy of most content**: CLI subcommands (`run`, `detect`, `probe`, `once`, `render`, `test-pattern`) match the actual `--help` output exactly. Config keys (`usb`, `profile`, `source`, `prometheus`, `api`, `render`, `tiles`) match the real `config.json`. File references (`config/config.json`, `config/config.sensors.json`, `Dockerfile`, `docker-compose.yml`, `packaging/systemd/trcc-display.service`, `packaging/udev/`) all exist.
- **Config table is useful and self-documented**: The `$comment` convention in JSON configs is well-explained. Unknown-key-tolerance is a nice touch for users.
- **Professional tone and formatting**: Clean markdown, consistent heading hierarchy, well-formatted tables, inline code blocks, and ASCII architecture diagram.
- **License attribution handled properly**: The NOTICE file correctly attributes the reverse-engineered protocol to `trcc-linux`, and the README acknowledges this in the License section.
- **CI/release claims verified**: GitHub Actions workflows (`ci.yml`, `release.yml`) match the documented jobs — `cargo fmt --check`, `cargo test`, `cargo clippy`, aarch64 cross-compile, Docker to GHCR with semver/minor/latest tags, and GitHub Release with checksums.

### Cons / Suggestions

- **Bug — Wrong preview URL in Quick Start** (`README.md` line ~78): The "With live web preview" example says `open http://localhost:9110/` but the actual preview is at `http://localhost:9110/preview`. This is confirmed by `src/config.rs:130`, `src/api.rs:77`, and `src/preview.rs:41`. This would confuse users trying the live preview for the first time. **Severity: Medium** — a broken URL in the primary "try it now" example.

- **Missing installation instructions for pre-built binaries**: The README covers building from source and running Docker, but the release workflow produces downloadable x86_64/aarch64 `.tar.gz` archives on GitHub Releases. There's no "Install a release binary" section for users who don't want to build from source. **Severity: Low** — most CLI Rust users can `cargo build`, but it would be a nice addition.

- **Minor — `render` example is slightly incomplete**: The README shows `--slot gpu_temp --value 63 --unit celsius --color 255,60,60` but doesn't mention that `--config` is also required (defaults to `config.json`). This is consistent with the actual CLI, but since `render` doesn't need a metric source, the example could explicitly show `--config config/config.json` for clarity. **Severity: Low** — the default is documented in the `--help` text.

- **Minor — `test-pattern` example omits `--color` and `--delay-ms` options**: The README only mentions `--mode walk|all`. The actual CLI also supports `--color r,g,b` and `--delay-ms` (default 250). Not critical, but these options are useful for profile mapping. **Severity: Low**.

---

## Review Findings Summary

| Finding | Location | Severity | Status |
|---------|----------|----------|--------|
| Wrong preview URL in Quick Start (`http://localhost:9110/` → should be `http://localhost:9110/preview`) | `README.md` ~line 78 | Medium | Actionable fix |
| No "install from release binary" section | `README.md` | Low | Suggestion |
| `render` example could be more explicit about `--config` | `README.md` CLI section | Low | Suggestion |
| `test-pattern` omits `--color` and `--delay-ms` flags | `README.md` CLI section | Low | Suggestion |

No blockers. The README is high-quality overall with one factual URL error to fix.

---