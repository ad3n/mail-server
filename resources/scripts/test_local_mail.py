#!/usr/bin/env python3
"""Kirim email ke akun Stalwart sendiri lalu periksa INBOX melalui IMAP."""

import argparse
import getpass
import http.client
import imaplib
import json
import smtplib
import ssl
import sys
import time
import uuid
from email import message_from_bytes
from email.message import EmailMessage
from email.utils import formatdate
from pathlib import Path

HOST = "127.0.0.1"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("account", nargs="?", help="Alamat email lengkap untuk login, pengirim, dan penerima")
    parser.add_argument("--status", type=uuid.UUID, metavar="ID", help="Cek ulang ID tes sebelumnya")
    parser.add_argument("--smtp-port", type=int, default=1465)
    parser.add_argument("--imap-port", type=int, default=1993)
    parser.add_argument("--self-signed", action="store_true", help="Lewati verifikasi sertifikat TLS lokal")
    args = parser.parse_args()

    context = ssl.create_default_context()
    if args.self_signed or args.account is None:
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE

    test_id = args.status or uuid.uuid4()
    message_id = f"<{test_id}@stalwart-test.localhost>"
    local_dir = Path(__file__).resolve().parents[2] / ".stalwart-local"
    if args.account is None:
        args.account = "webhook-test@local.test"
        password = (local_dir / "webhook-test-password").read_text().rstrip("\r\n")
    else:
        password = getpass.getpass(f"Password {args.account}: ")
    headers = json.loads((local_dir / "test-mail-headers.json").read_text())
    app_headers = {"App-Key": headers["App-Key"], "App-Token": headers["Token-Key"]}
    if not args.status:
        mail = EmailMessage()
        mail["From"] = mail["To"] = args.account
        mail["Message-ID"] = message_id
        for name, value in app_headers.items():
            mail[name] = value
        mail["Token-Key"] = headers["Token-Key"]
        mail["External-ID"] = str(test_id)
        mail["Date"] = formatdate(localtime=True)
        mail["Subject"] = f"Tes Stalwart lokal {test_id}"
        mail.set_content("Pesan ini dikirim untuk menguji pengiriman ke INBOX lokal.")
        print(f"ID tes: {test_id}", flush=True)
        with smtplib.SMTP_SSL(HOST, args.smtp_port, context=context, timeout=10) as smtp:
            smtp.login(args.account, password)
            smtp.send_message(mail, from_addr=args.account, to_addrs=[args.account])
            print("DITERIMA SMTP", flush=True)

    with imaplib.IMAP4_SSL(HOST, args.imap_port, ssl_context=context, timeout=10) as imap:
        imap.login(args.account, password)
        deadline = time.monotonic() + 30
        while True:
            result, count = imap.select("INBOX", readonly=True)
            if result != "OK":
                raise RuntimeError("Gagal membuka INBOX")
            result, matches = imap.uid("SEARCH", None, "HEADER", "Message-ID", message_id)
            if result != "OK":
                raise RuntimeError("Pencarian IMAP gagal")
            if matches[0]:
                print(f"MASUK INBOX (UID {matches[0].decode()})")
                return show_tracking(test_id, app_headers)
            # Header lookup can lag delivery while the search index updates.
            total = int(count[0])
            if total:
                recent = f"{max(1, total - 24)}:{total}"
                result, headers = imap.fetch(recent, "(BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])")
                if result != "OK":
                    raise RuntimeError("Gagal membaca header INBOX")
                for item in headers:
                    if isinstance(item, tuple) and message_from_bytes(item[1])["Message-ID"] == message_id:
                        print("MASUK INBOX")
                        return show_tracking(test_id, app_headers)
            if args.status or time.monotonic() >= deadline:
                print(f"BELUM TERLIHAT DI INBOX. Cek ulang dengan --status {test_id}")
                return 2
            time.sleep(1)


def show_tracking(test_id, headers):
    connection = http.client.HTTPConnection(HOST, 8080, timeout=10)
    try:
        connection.request("GET", f"/api/integrations/deliveries/{test_id}/status", headers=headers)
        response = connection.getresponse()
        if response.status == 200:
            print("Status pengiriman:", response.read().decode())
            return 0
        if response.status == 404:
            print(f"Status belum tercatat. Cek ulang dengan ./tes-email --status {test_id}")
            return 2
        print(f"Gagal membaca status integrasi (HTTP {response.status}).")
        return 1
    finally:
        connection.close()


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, imaplib.IMAP4.error, RuntimeError, ValueError) as error:
        sys.exit(f"Gagal: {error}")
