#!/usr/bin/env bash
# Installe ou met à jour l'API CollectionOps sur un LXC Debian/Ubuntu avec systemd.
set -Eeuo pipefail

readonly SERVICE=collectionops-dev
readonly BINARY=/usr/local/bin/collectionops-backend
readonly UNIT=/etc/systemd/system/collectionops-dev.service
readonly ENV_FILE=/etc/collectionops/dev.env
readonly REPO=https://github.com/lolo1580/CollectionOps.git
readonly REF=main

if (( EUID != 0 )); then
  echo 'Exécuter ce script en root sur le LXC.' >&2
  exit 1
fi

for command in git cargo install systemctl curl mktemp; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Commande manquante : $command" >&2
    exit 1
  fi
done

if [[ ! -f "$ENV_FILE" ]]; then
  echo "Configuration absente : $ENV_FILE" >&2
  echo 'Créez ce fichier (mode 600) avec COLLECTIONOPS_DATABASE_URL et COLLECTIONOPS_BIND, puis relancez.' >&2
  exit 1
fi
if ! grep -q '^COLLECTIONOPS_DATABASE_URL=mysql://' "$ENV_FILE"; then
  echo "COLLECTIONOPS_DATABASE_URL doit pointer vers MariaDB dans $ENV_FILE." >&2
  exit 1
fi

if [[ -e "$UNIT" ]] && ! grep -Fq "ExecStart=$BINARY" "$UNIT"; then
  echo "Le service existant n'utilise pas $BINARY ; installation interrompue." >&2
  exit 1
fi

echo 'Avant une mise à jour, sauvegardez la base MariaDB : les migrations ne sont pas réversibles automatiquement.'
read -r -p 'Sauvegarde vérifiée ? Tapez OUI pour continuer : ' answer </dev/tty
if [[ "$answer" != OUI ]]; then
  echo 'Installation annulée, aucun service modifié.'
  exit 1
fi

work_dir=$(mktemp -d /tmp/collectionops-install.XXXXXX)
backup_binary=
installed=0
cleanup() {
  if [[ "$work_dir" == /tmp/collectionops-install.* && -d "$work_dir" ]]; then
    rm -rf -- "$work_dir"
  fi
}
rollback() {
  local status=$?
  if (( status != 0 && installed == 1 )) && [[ -n "$backup_binary" ]]; then
    echo 'Échec : restauration de l’ancien exécutable et redémarrage du service.' >&2
    install -m 755 "$backup_binary" "$work_dir/collectionops-backend.restore"
    mv -f "$work_dir/collectionops-backend.restore" "$BINARY"
    systemctl restart "$SERVICE" || true
  fi
  cleanup
}
trap rollback EXIT

git clone --quiet --depth 1 --branch "$REF" "$REPO" "$work_dir/source"
cargo build --manifest-path "$work_dir/source/Cargo.toml" -p collectionops-backend --release --locked

if [[ -f "$BINARY" ]]; then
  backup_binary="$work_dir/collectionops-backend.previous"
  install -m 755 "$BINARY" "$backup_binary"
fi

if [[ ! -e "$UNIT" ]]; then
  install -d -m 755 /etc/systemd/system
  tee "$UNIT" >/dev/null <<'EOF'
[Unit]
Description=CollectionOps API (développement)
Wants=network-online.target
After=network-online.target mariadb.service

[Service]
Type=simple
DynamicUser=yes
EnvironmentFile=/etc/collectionops/dev.env
ExecStart=/usr/local/bin/collectionops-backend
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable "$SERVICE"
fi

install -m 755 "$work_dir/source/target/release/collectionops-backend" "$work_dir/collectionops-backend.new"
mv -f "$work_dir/collectionops-backend.new" "$BINARY"
installed=1
systemctl restart "$SERVICE"

bind=$(grep '^COLLECTIONOPS_BIND=' "$ENV_FILE" | tail -n 1 | cut -d= -f2- || true)
bind=${bind:-127.0.0.1:8080}
host=${bind%:*}
port=${bind##*:}
if [[ ! "$host" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ || ! "$port" =~ ^[0-9]+$ ]]; then
  echo "COLLECTIONOPS_BIND non pris en charge pour la sonde : $bind" >&2
  exit 1
fi
if [[ "$host" == 0.0.0.0 ]]; then
  host=127.0.0.1
fi

ready=0
for (( attempt=0; attempt<20; attempt++ )); do
  if curl --fail --silent --max-time 2 "http://$host:$port/api/v1/health/ready" >/dev/null; then
    ready=1
    break
  fi
  sleep 1
done
if (( ready != 1 )); then
  echo 'Le service ne répond pas à /health/ready. Vérifiez : journalctl -u collectionops-dev -n 80 --no-pager' >&2
  exit 1
fi

installed=0
echo "CollectionOps installé depuis $REPO ($REF)."
echo "API prête sur le port $port ; configuration conservée dans $ENV_FILE."
