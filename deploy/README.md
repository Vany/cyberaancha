# Deploy

Routine: `make deploy` (build musl binary on the Mac, ship, rebuild scratch image on n1, restart, smoke /healthz). `make logs` tails the container.

## One-time on n1 (needs sudo — Vany)

**DNS first.** Create `youtube.serezhkin.com` as a CNAME → `n1.serezhkin.com`
(the pattern `music.serezhkin.com` already uses → 164.92.213.60). Do NOT let it
resolve to the `www` box (159.69.146.250) — that's a different server and
certbot's HTTP-01 challenge would fail there. Confirm with:

```sh
dig +short @8.8.8.8 youtube.serezhkin.com   # must show 164.92.213.60
```

Then (test host shown; production later is identical with `aancha.serezhkin.com`).
**Symlink, don't copy** — the vhost in `~vany/cyberaancha/nginx-cyberaancha.conf` stays the
single source you fully control; `make deploy` never overwrites it (ships only if
absent), and certbot edits it in place:

```sh
sudo ln -s ~vany/cyberaancha/nginx-cyberaancha.conf /etc/nginx/sites-enabled/youtube.serezhkin.com
sudo nginx -t && sudo systemctl reload nginx
sudo certbot --nginx -d youtube.serezhkin.com   # per-host, HTTP-01, auto-renews via certbot.timer
```

certbot writes the `443` TLS server block straight into `~vany/cyberaancha/nginx-cyberaancha.conf`
(the symlink target). To edit the vhost afterwards, edit that file on n1 directly —
it's yours; the repo copy is only the first-run template. Wildcard note:
`*.serezhkin.com` would need DNS-01 + a DNS-API plugin to auto-renew — avoid unless
many subdomains are planned; per-host is simpler here.

## Credentials (once, on n1)

```sh
cd ~/cyberaancha
echo 'CHOSEN_ADMIN_PASSWORD' | docker compose exec -T cyberaancha /cyberaancha-server --config /app/cyberaancha.toml set-password admin
echo 'CHOSEN_OWNER_PASSWORD' | docker compose exec -T cyberaancha /cyberaancha-server --config /app/cyberaancha.toml set-password owner
docker compose exec -T cyberaancha /cyberaancha-server --config /app/cyberaancha.toml gen-token collector   # prints once
```

## Restore (destructive)

```sh
cd ~/cyberaancha && docker compose down
docker compose run --rm cyberaancha restore --latest --yes
docker compose up -d
```
