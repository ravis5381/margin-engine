# Ship the server to another machine

Build the image on this machine, copy one archive, and start Postgres and the
server from that archive. The other machine needs Docker. It does not need
this source tree or a Rust toolchain.

The database password below is the local Compose password. Change
`POSTGRES_PASSWORD` and the password inside `DATABASE_URL` together before
the first start, and keep them the same after that. The Postgres volume keeps
the first password.

## On this machine

From the repository root:

The other machine is `linux/amd64`. This Mac builds `linux/arm64` unless BuildKit is asked for that platform. The legacy builder keeps the ARM base image and then fails with `does not provide the specified platform (linux/amd64)`. An arm64 image exits with `exec format error` on the other host.

```bash
cd /Users/ravisingh/Documents/margin_calculator

docker buildx build --platform linux/amd64 --pull --load -t margin-engine:0.1.0 .
docker pull --platform linux/amd64 postgres:16

mkdir -p dist
docker save margin-engine:0.1.0 postgres:16 | gzip > dist/margin-engine-0.1.0.tar.gz
```

Copy the archive. Replace the destination with the other machine:

```bash
scp dist/margin-engine-0.1.0.tar.gz user@other-host:/opt/margin-engine/
```

## On the other machine

```bash
mkdir -p /opt/margin-engine
cd /opt/margin-engine
docker load < margin-engine-0.1.0.tar.gz

docker network create margin
docker volume create margin_pg
docker volume create margin_span_index
docker run --rm --user root --entrypoint chown \
  -v margin_span_index:/data \
  margin-engine:0.1.0 10001:10001 /data

docker run -d --name margin-postgres --network margin \
  -e POSTGRES_USER=margin \
  -e POSTGRES_PASSWORD=margin \
  -e POSTGRES_DB=margin_engine \
  -v margin_pg:/var/lib/postgresql/data \
  postgres:16

until docker exec margin-postgres pg_isready -U margin -d margin_engine; do
  sleep 2
done

docker run -d --name margin-engine --network margin \
  -p 0.0.0.0:8080:8080 \
  -w /home/margin \
  -v margin_span_index:/home/margin/data \
  -e DATABASE_URL=postgres://margin:margin@margin-postgres:5432/margin_engine \
  -e BIND_ADDR=0.0.0.0:8080 \
  -e RUST_LOG=info,sqlx=warn \
  -e MARGIN_LOG_FORMAT=json \
  margin-engine:0.1.0
```

Migrations run when the server connects. The process binds `0.0.0.0:8080`, and Docker publishes that port on every address of this machine. Another computer reaches it at `http://<this-machine>:8080`.

```bash
curl -sS http://127.0.0.1:8080/health
```

`GET /ready` stays unavailable. A quote for a date that is not already in
Postgres downloads that day's official files, so this machine needs outbound
HTTPS to `www.nseindia.com`, `www.bseindia.com`, and `api.bseindia.com`. The
first quote for a new date takes longer because those files are fetched then.

```bash
curl -sS -m 180 -H 'content-type: application/json' \
  -d '{"exchange":"BSE","trading_datetime":"2026-10-16T10:00:00+05:30","positions":[{"underlying":"SENSEX","expiry":"2026-11-26","right":"FUT","quantity":"-20"}]}' \
  http://127.0.0.1:8080/api/v1/margin/calculate
```

Swagger UI is at `http://<this-machine>:8080/swagger-ui`. From this machine, `127.0.0.1` reaches the same port.

## Stop and start again

```bash
docker stop margin-engine margin-postgres
docker start margin-postgres
until docker exec margin-postgres pg_isready -U margin -d margin_engine; do
  sleep 2
done
docker start margin-engine
```

The Postgres volume and the SPAN index volume keep their data across a restart.
