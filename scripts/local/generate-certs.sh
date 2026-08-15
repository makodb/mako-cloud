#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)
cert_dir="$project_root/.local/certs"
openssl_config="$project_root/infra/local/openssl.cnf"

mkdir -p "$cert_dir"

openssl req \
  -x509 \
  -newkey rsa:3072 \
  -sha256 \
  -days 3650 \
  -nodes \
  -subj "/CN=Mako Cloud Local CA/O=Mako Cloud Local Development" \
  -keyout "$cert_dir/ca.key" \
  -out "$cert_dir/ca.crt"

openssl req \
  -new \
  -newkey rsa:3072 \
  -sha256 \
  -nodes \
  -config "$openssl_config" \
  -keyout "$cert_dir/server.key" \
  -out "$cert_dir/server.csr"

openssl x509 \
  -req \
  -sha256 \
  -days 825 \
  -in "$cert_dir/server.csr" \
  -CA "$cert_dir/ca.crt" \
  -CAkey "$cert_dir/ca.key" \
  -CAcreateserial \
  -extfile "$openssl_config" \
  -extensions request_extensions \
  -out "$cert_dir/server.crt"

chmod 600 "$cert_dir/ca.key" "$cert_dir/server.key"
openssl verify -CAfile "$cert_dir/ca.crt" "$cert_dir/server.crt"

