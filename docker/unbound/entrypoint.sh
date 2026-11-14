#!/bin/sh
set -eu

if [ ! -s /var/lib/unbound/root.key ]; then
    cp /usr/share/dns/root.key /var/lib/unbound/root.key
fi
chown unbound:unbound /var/lib/unbound /var/lib/unbound/root.key
unbound-checkconf /etc/unbound/unbound.conf
exec unbound -d -c /etc/unbound/unbound.conf
