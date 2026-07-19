TARGET := x86_64-unknown-linux-musl
BIN    := target/$(TARGET)/release/aancha-server

.PHONY: build-linux deploy logs test

test:
	cargo test

build-linux:
	cargo zigbuild --release --target $(TARGET)

deploy: build-linux
	ssh n1 'mkdir -p ~/aancha/data ~/aancha/index ~/aancha/backups'
	scp $(BIN) deploy/Dockerfile deploy/docker-compose.yml n1:~/aancha/
	# Config + nginx vhost are server-owned: ship only if absent, never clobber.
	# certbot edits the vhost in place; the sites-enabled symlink points here.
	ssh n1 'test -f ~/aancha/aancha.toml' || scp aancha.toml.example n1:~/aancha/aancha.toml
	ssh n1 'test -f ~/aancha/nginx-aancha.conf' || scp deploy/nginx-aancha.conf n1:~/aancha/nginx-aancha.conf
	ssh n1 'cd ~/aancha && docker compose up -d --build'
	@sleep 2
	ssh n1 'curl -fsS http://127.0.0.1:8087/healthz' && echo " <- n1 healthz"

logs:
	ssh n1 'cd ~/aancha && docker compose logs --tail 50 -f'
