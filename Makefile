TARGET := x86_64-unknown-linux-musl
BIN    := target/$(TARGET)/release/cyberaancha-server

.PHONY: build-linux deploy logs test

test:
	cargo test

build-linux:
	cargo zigbuild --release --target $(TARGET)

deploy: build-linux
	ssh n1 'mkdir -p ~/cyberaancha/data ~/cyberaancha/index ~/cyberaancha/backups'
	scp $(BIN) deploy/Dockerfile deploy/docker-compose.yml n1:~/cyberaancha/
	# Config + nginx vhost are server-owned: ship only if absent, never clobber.
	# certbot edits the vhost in place; the sites-enabled symlink points here.
	ssh n1 'test -f ~/cyberaancha/cyberaancha.toml' || scp cyberaancha.toml.example n1:~/cyberaancha/cyberaancha.toml
	ssh n1 'test -f ~/cyberaancha/nginx-cyberaancha.conf' || scp deploy/nginx-cyberaancha.conf n1:~/cyberaancha/nginx-cyberaancha.conf
	ssh n1 'cd ~/cyberaancha && docker compose up -d --build'
	@sleep 2
	ssh n1 'curl -fsS http://127.0.0.1:8087/healthz' && echo " <- n1 healthz"

logs:
	ssh n1 'cd ~/cyberaancha && docker compose logs --tail 50 -f'
