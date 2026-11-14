FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends unbound dns-root-data dnsutils ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY unbound.conf /etc/unbound/unbound.conf
COPY entrypoint.sh /usr/local/bin/unbound-entrypoint
RUN chmod 755 /usr/local/bin/unbound-entrypoint

ENTRYPOINT ["/usr/local/bin/unbound-entrypoint"]
