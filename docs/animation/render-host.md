# Animation render host setup

Two render paths share a host today and a third lands in Phase G:

| Path | Driver | Toolchain | Setup doc |
|---|---|---|---|
| Revideo (default) | `backend/render/` (Node) | `node`, ffmpeg, system fonts | `backend/render/README.md` |
| ffmpeg fast path (F.1c) | inline in `backend/api/` | ffmpeg with libass + zoompan | already on every render host |
| Manim diagram path (G.3+) | `backend/manim/` (Python) | Python 3.11+, ffmpeg, Cairo, Pango, **LaTeX** | this doc |

This file is the canonical reference for **the Manim path**. Other
paths are documented in their own packages.

---

## TL;DR

- **Dev box (native install on Arch/EndeavourOS):** `pacman` for
  system deps, `uv` for the Python venv, `just manim-smoke` to
  verify.
- **Prod / CI:** build the OCI image from `backend/manim/Containerfile`
  with `podman build`, ship it. The image is ~3 GB (LaTeX + Cairo +
  Manim).

You can run the smoke either way:

```sh
# native
just manim-smoke

# containerised
just manim-container-smoke
```

Both produce a 1080p30 MP4. If the smoke succeeds, the path is ready
to wire into G.5 / G.6.

---

## Native install (dev)

### Arch / EndeavourOS

The fast path is the install script — it does the same thing the
manual recipe below does, but also installs `uv`, runs `uv sync`,
and verifies with the smoke render:

```sh
scripts/manim-install.sh                # full install + smoke
scripts/manim-install.sh --no-smoke     # install only
scripts/manim-install.sh --no-system    # skip pacman, just venv + smoke
```

If you'd rather drive it by hand:

```sh
sudo pacman -S --needed \
    python ffmpeg cairo pango \
    texlive-basic texlive-latexrecommended texlive-latexextra \
    texlive-pictures texlive-fontsrecommended \
    texlive-mathscience texlive-binextra
```

What each provides:

- `texlive-latexrecommended`: `amsmath`, `amssymb`, `babel`.
- `texlive-latexextra`: `standalone.cls` — Manim's default `\documentclass[preview]{standalone}`. Skipping this gives `LaTeX Error: File 'standalone.cls' not found` on the first `MathTex`.
- `texlive-pictures`: TikZ + PGF; some Manim text paths trip into it.
- `texlive-mathscience`: `physics`, `siunitx`, `mathtools` — common STEM macros.
- `texlive-binextra`: `dvisvgm` (DVI→SVG step).

The combination is ~400 MB on disk; budget accordingly.

### Debian / Ubuntu

```sh
sudo apt install -y --no-install-recommends \
    ffmpeg \
    libcairo2 libcairo2-dev \
    libpango-1.0-0 libpango1.0-dev \
    texlive-latex-base texlive-latex-extra \
    texlive-fonts-recommended texlive-science \
    dvisvgm
```

(Same package set the `Containerfile` installs — kept here so a
non-container box can match the prod environment exactly.)

### macOS

```sh
brew install python ffmpeg cairo pango
brew install --cask mactex-no-gui
```

`mactex-no-gui` is the LaTeX distribution; `mactex` (with apps) also
works but adds 1 GB of UI you don't need on a render host.

### Fedora / RHEL

```sh
sudo dnf install -y \
    python3 python3-pip ffmpeg \
    cairo cairo-devel pango pango-devel \
    texlive-scheme-medium dvisvgm
```

### Python venv (any distro)

```sh
cd backend/manim
uv sync                       # creates .venv from pyproject.toml
uv run listenai-manim-smoke   # runs the smoke (~30 s first time)
```

If `uv` isn't installed:

```sh
# Either install it once (recommended):
curl -LsSf https://astral.sh/uv/install.sh | sh

# Or fall back to plain pip:
python -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
pip install -e .
listenai-manim-smoke
```

The first run is slow (~30 s) because Manim warms its LaTeX cache.
Subsequent runs are 5–15 s for the smoke and 0.5–3× realtime for
real diagrams.

---

## Container install (prod / CI / reproducible dev)

The project standardises on **podman** — the `Containerfile` is OCI-
compliant so docker works equally if you have it, but the
`just manim-container-*` recipes call `podman` directly.

### Build

```sh
just manim-container-build       # ~3–5 min cold, ~30 s incremental
# == podman build -f backend/manim/Containerfile -t listenai-manim:0.1 backend/manim
```

The image lands at the local registry as `listenai-manim:0.1`.
Inspect with `podman images listenai-manim`. Expect ~3 GB on disk
(LaTeX is the biggest single contributor — texlive-* layers total
~2 GB).

### Run the smoke

```sh
just manim-container-smoke
# == podman run --rm \
#       -v $PWD/backend/manim/smoke_output:/home/manim/smoke_output:Z \
#       listenai-manim:0.1 python -m listenai_manim.smoke
```

The `:Z` SELinux relabel flag is harmless on systems without SELinux
and required on Fedora / RHEL / EndeavourOS-with-SELinux. Drop it if
your distro doesn't ship SELinux at all.

### Run a full render (G.5+)

Once G.5 ships the NDJSON server, the production invocation becomes:

```sh
podman run --rm \
    -v $STORAGE:/storage:Z \
    -i listenai-manim:0.1 \
    python -m listenai_manim.server < spec.ndjson
```

The Rust publisher manages this — operators just need the image
present on the worker host.

---

## Operational notes

- **Image size budget:** ~3 GB. Plan a 5 GB free disk minimum on
  worker nodes; older versions linger until you `podman image prune`.
- **First-render latency:** Manim caches LaTeX font glyphs to
  `~/.cache/texlive` on first use; that's a one-time 5–10 s cost.
  In the container the cache is per-container instance unless you
  mount a volume — for prod, mount `/home/manim/.cache` to a named
  volume so subsequent containers warm-start.
- **GPU:** Manim is CPU-only. The F.1f.1 hwenc auto-detect doesn't
  help here — Manim does its encode through its own ffmpeg pipeline.
  If we want hwenc later we'd patch Manim's encoder config; not in
  scope for G.3.
- **Concurrency:** one Python interpreter per Manim render in v1
  (G.5's `server.py` reuses the interpreter across many renders to
  amortize the ~3 s import cost). The Rust pool sizes itself to
  `LISTENAI_ANIMATE_CONCURRENCY`.
- **Memory:** ~400–600 MB RSS during a render, ~150 MB idle. Budget
  roughly the same as the Revideo sidecar.

## Troubleshooting

**`! LaTeX Error: File 'physics.sty' not found.`** — `texlive-science`
(Debian) / `texlive-mathscience` (Arch) is missing. Install it.

**`OSError: cannot load library 'libpango-1.0.so.0'`** — the Pango
runtime isn't installed (only the dev headers were, or vice versa).
On Debian: `apt install libpango-1.0-0 libpango1.0-dev`.

**`ManimGenericError: dvisvgm not found on PATH`** — install
`dvisvgm` explicitly. On Arch it ships with `texlive-binextra`; on
Debian it's a separate package.

**Container builds fail on EndeavourOS with podman.** — most often
SELinux/AppArmor labelling on bind-mounts. Append `:Z` to every
volume mount; if that's not enough, `podman unshare chown -R 1000:1000
backend/manim/smoke_output` once.

**Smoke MP4 is 0 bytes.** — Manim wrote frames but ffmpeg failed to
mux. Check `backend/manim/smoke_output/Tex/` for stale LaTeX builds
that block fresh ones; clear it with `rm -rf backend/manim/smoke_output`.

**`ImportError: /usr/lib/libpangoft2-1.0.so.0: undefined symbol: FcConfigSetDefaultSubstitute`.** —
The manimpango wheel from PyPI bundles its own `libfontconfig.so.1`
(via auditwheel), older than what Arch's current `pango` requires.
The wheel's bundled lib gets loaded first via `RPATH`, missing the
symbol that system pango needs. Fix is to force-load the system
libfontconfig ahead of the bundle:

```sh
LD_PRELOAD=/usr/lib/libfontconfig.so.1 uv run listenai-manim-smoke
```

The `manim-smoke` justfile recipe (and `scripts/manim-install.sh`)
already does this. If you build a new venv by hand, prepend
`LD_PRELOAD=/usr/lib/libfontconfig.so.1` to your run command.

**`Failed to build manimpango==X.Y.Z` with `cannot find -lpango-1.0`.** —
Active conda env leaks two things into uv's build subprocess:

1. **Conda's Python 3.13+** — `manimpango` only ships wheels for
   CPython 3.9–3.12, so 3.13 forces a source build.
2. **Conda's compiler env vars** (`CC`, `CXX`, `LDFLAGS`, …) — these
   route the source build through `x86_64-conda-linux-gnu-cc`, which
   only knows about `/home/<you>/miniconda3/lib`. System pango/cairo
   live in `/usr/lib`, so the link fails.

`scripts/manim-install.sh` defends against both: it pins uv to a
managed Python 3.11 (`.python-version` + `--python-preference
managed`) and runs `uv sync` inside an `env -u CC -u CXX -u LDFLAGS
…` wrapper that strips the conda toolchain vars and re-prepends
`/usr/bin` to PATH. `pyproject.toml` also sets `[tool.uv] only-binary
= ["manimpango", "pycairo"]` so uv refuses to source-build them at
all — the prebuilt wheels work everywhere.

If you're driving `uv sync` by hand, copy the env wrapper from
`scripts/manim-install.sh` (or run `just manim-build`, which embeds
it). A `conda deactivate` ahead of the run also works but only for
the immediate shell.

Pinning to 3.11 also covers a future where Manim drops 3.12 wheels
before adding 3.14 ones — the version cap in `pyproject.toml`
(`requires-python = ">=3.11,<3.13"`) makes that mismatch visible
at sync time, not at build time.
