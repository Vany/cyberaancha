# Deploy

Routine: `make deploy` (build musl binary on the Mac, ship, rebuild scratch image on n1, restart, smoke /healthz). `make logs` tails the container.

## One-time on n1 (needs sudo — Vany)

After the DNS A-record `aancha.serezhkin.com` → n1 exists:

```sh
sudo cp ~vany/aancha/nginx-aancha.conf /etc/nginx/sites-available/aancha.serezhkin.com
sudo ln -s /etc/nginx/sites-available/aancha.serezhkin.com /etc/nginx/sites-enabled/
sudo nginx -t && sudo systemctl reload nginx
sudo certbot --nginx -d aancha.serezhkin.com
```

## Credentials (once, on n1)

```sh
cd ~/aancha
echo 'CHOSEN_ADMIN_PASSWORD' | docker compose exec -T aancha /aancha-server --config /app/aancha.toml set-password admin
echo 'CHOSEN_OWNER_PASSWORD' | docker compose exec -T aancha /aancha-server --config /app/aancha.toml set-password owner
docker compose exec -T aancha /aancha-server --config /app/aancha.toml gen-token collector   # prints once
```

## Restore (destructive)

```sh
cd ~/aancha && docker compose down
docker compose run --rm aancha restore --latest --yes
docker compose up -d
```
