# Third-party notices

## libghostty-vt 1.0.0

Statically linked into every binary that draws or parses a terminal.
MIT License, Copyright (c) 2024 Mitchell Hashimoto, Ghostty contributors.
Source: https://release.files.ghostty.org/libghostty-vt/1.0.0/

It bundles:

| Component | Version | License |
|---|---|---|
| Highway | 1.2.0 | Apache-2.0 OR BSD-3-Clause |
| simdutf | 9.0.0 | Apache-2.0 OR MIT |
| uucode | 0.2.0 | MIT AND Unicode-3.0 |
| Wuffs | 0.4.0-alpha.10 | Apache-2.0 OR MIT |
| translate_c | 2.0.0 | MIT |

The full machine-readable list is `deps/libghostty-vt-third-party.json` in the
source tarball.

## Herdr

Design ideas (daemon-owned PTYs, OSC 7501 status, the agent list ordering)
come from Herdr, Herdr Desktop and herdr-core (Apache-2.0,
https://github.com/herdrdev). No Herdr source code is included.
