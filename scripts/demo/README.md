# VHS product demo

Record from the repository root:

```sh
brew install vhs
python3 scripts/demo/record.py
```

Requires Python 3, Rust (`cargo` and `rustc`), and a notagent binary on `PATH`. To record a
specific build, pass `--binary target/release/notagent`. VHS also needs `ttyd`
and `ffmpeg`; Homebrew installs them with VHS. VHS may download a browser on
its first run. See the [VHS documentation](https://github.com/charmbracelet/vhs).

The output is `assets/demo/notagent.gif`, with an MP4 and two PNG checkpoints
alongside it. Edit `notagent.tape` for dimensions, typing speed and reading pauses.

The demo shows a shipping threshold bug: inspect with `read_minified`, plan,
switch from plan to auto with Shift+Tab twice, apply `patch_minified`, then
compile and run three real Rust tests. The recorder checks that the only source
change is `>` to `>=` and that the actual tool output reports three passing tests.

This is a scripted demonstration, not a live model benchmark. A loopback server
provides fixed model responses, labeled `Scripted demo` in the UI. The notagent
TUI, agent loop, file tools, diff and tests are real. Timing is chosen for
readability and does not represent provider latency or token costs.

Every run creates a temporary project and separate configuration, disables
startup network operations and telemetry, and removes the temporary files on
exit. It does not use personal provider credentials or modify the working
project. Atomic leases are enabled in the demo configuration. Bash filtering is
disabled so the short test result stays fully visible. The recording does not
claim measured savings or show concurrent writers.
