# CLAUDE.md — cyberaancha

Knowledge base + bot around Prof. Ancha Baranova's YouTube channel. Read order: **SPEC.md → PROG.md → TODO.md → MEMO.md** (MEMO newest-first; update it at every task finish).

## Iron rules (from SPEC, short form)
- **No LLM in production.** The server only searches and templates. All intelligence happens at build time on the Mac (Claude sessions + scripts).
- **The server never talks to YouTube.** Harvesting = collector JS in a browser page context; audio = yt-dlp on the Mac only.
- Scientific reference base: quote and attribute the professor, never synthesize medical advice.
- Fail loudly; validate at boundaries; reject, don't repair.

## Facts
- Server: n1.serezhkin.com (`ssh n1`, IP 164.92.213.60), deploy dir `~vany/aancha`, app on 127.0.0.1:8087 behind existing nginx + Let's Encrypt. **Test host: https://youtube.serezhkin.com** (Vany's channel); prod later: https://aancha.serezhkin.com (her channel). Subdomains must CNAME → n1.serezhkin.com, NOT the www box (159.69.146.250). Box: 1 vCPU / 457 MB / disk tight — never compile there, memory-cap the container.
- Build Mac: M4 Max. `make build-linux` (cargo-zigbuild → x86_64-musl) → `make deploy`.
- Test channel: @vanyserezhkin (in config); production: @AnchaBaranovaProf. Harvest in 7-day windows.
- Secrets: DB `auth` table via CLI (`set-password`, `gen-token`). Nothing secret in config or git.
- Repo language: English (KB content itself is Russian). gitmode: history (commit to main).

## Commands
- `cargo run -- serve --config aancha.toml` — local dev
- `cargo test` — keep fast
- `aancha-server backup` / `restore --latest --yes` (destructive)
