# Report collector

Where the reports from the picker's two menus (docs/protocol.md "report") end up.

```
client (emoticond serve)  --HTTPS-->  Cloudflare tunnel  -->  Traefik  -->  emoticond-collector  -->  SQLite
                                      emoticond.mewo.gay                   (container, port 8080)    /data/reports.db
```

## Client side

- A report is queued in `$XDG_STATE_HOME/emoticond/reports/queue.jsonl` the
  moment it is made, and its local effects apply at once (hide, demote,
  pick). Sending is separate and never blocks the picker.
- `emoticond serve` starts a background sender: at start and then every
  minute it sends the pending reports that are **at least 2 minutes old**,
  so a report undone or changed within that time never leaves the machine.
  Only the last report per (query, face, slot) is sent, and a `clear` is
  sent only when it withdraws something already sent. Accepted reports are
  recorded in `reports/sent.jsonl`; the rest stay queued for the next try.
  The CLI (`emoticond report`) only queues; the next daemon sends.
- The endpoint is `feedback.endpoint`, default
  `https://emoticond.mewo.gay/v1/reports`. `EMOTICOND_REPORT_ENDPOINT`
  overrides it (`none` turns sending off; the tests set that). Sending is off when the user sets
  `feedback.send = false`, when the policy disables reports, and in a build
  without the `net` feature (`emoticond-cli` default features), which has no
  network code at all.

## Wire format

```
POST /v1/reports
{"reports": [Report, ...]}            at most 50, body at most 512 KiB

200 {"accepted": ["<report_id>", ...], "rejected": [{"index": 1, "error": "missing field `v`"}]}
400 {"error": "..."}                  not JSON, not {"reports":[...]}, too many
413 {"error": "request too large"}
429 {"error": "too many reports from here; try again later"}

GET /healthz  ->  ok
```

`Report` is the library type (`emoticond::Report`, report version 1). The
collector parses each one with that type and checks sizes (query, reading
and face text at most 500 chars, note at most 2000, `shown` at most 20).
Storing is idempotent on `report_id`: a resent report is accepted again, so
the client marks it sent, but kept once.

## Server side

- Code: `engine/crates/emoticond-collector` (tiny_http + SQLite, one
  thread). Dockerfile next to it; build context `engine/`.
- It listens on `$EMOTICOND_COLLECTOR_ADDR` (default `0.0.0.0:8080`) and
  stores to `$EMOTICOND_COLLECTOR_DB` (default `/data/reports.db`; the image
  declares `/data` a volume). TLS and the public hostname are the reverse
  proxy's job (the default endpoint runs behind a Cloudflare tunnel and
  Traefik).
- Nothing about the sender is stored: no IP, no headers. The client IP
  (`CF-Connecting-IP`, else `X-Forwarded-For`, else the peer) is only used in
  memory for the rate limit: 120 requests and 300 reports per client per
  hour.
- Table `reports`: `report_id` (key), `received_ms`, `ts`, `key`, `reason`,
  `query`, `face`, `data_version`, and the whole report as `json`.

## Operating it

```sh
docker build -f engine/crates/emoticond-collector/Dockerfile -t emoticond-collector engine
docker run -d --name emoticond-collector -p 8080:8080 -v emoticond-reports:/data emoticond-collector
curl http://localhost:8080/healthz
docker exec emoticond-collector emoticond-collector dump > reports.jsonl              # every report, JSONL
docker exec emoticond-collector emoticond-collector dump --since 1791400000000          # newer than (unix ms)
```

Point a client at it with `EMOTICOND_REPORT_ENDPOINT=http://localhost:8080/v1/reports`
or `feedback.endpoint` (a packager can set it in the policy's `[endpoints]`).

For the current state of a (query, face) choice, take the last report per
`key` and drop it if that last one is a `clear`.
