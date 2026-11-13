#!/usr/bin/env python3
"""Kirim email terlacak melalui SMTP remote dan periksa status pengirimannya."""

import argparse
import getpass
import http.client
import json
import os
import smtplib
import ssl
import sys
import time
import uuid
from email.message import EmailMessage
from email.utils import formatdate
from urllib.parse import urlsplit

from mail_tracking import external_id, outcome


def secret(name, label):
    value = os.environ.get(name) or getpass.getpass(f"{label}: ")
    if not value or any(char in value for char in "\r\n"):
        raise ValueError(f"{label} wajib diisi tanpa karakter baris baru")
    return value


def make_message(account, recipient, test_id, app_key, app_token):
    mail = EmailMessage()
    mail["From"] = account
    mail["To"] = recipient
    mail["Date"] = formatdate(localtime=True)
    mail["Message-ID"] = f"<{test_id}@{account.rsplit('@', 1)[-1]}>"
    mail["Subject"] = f"Tes email webhook {test_id}"
    mail["App-Key"] = app_key
    mail["App-Token"] = app_token
    mail["External-ID"] = test_id
    mail.set_content(f"Tes pengiriman email dan webhook Stalwart.\nExternal-ID: {test_id}\n")
    return mail


def send_mail(args, password, mail):
    context = ssl.create_default_context()
    port = args.smtp_port or (465 if args.smtp_security == "tls" else 587)
    if args.smtp_security == "tls":
        smtp = smtplib.SMTP_SSL(args.smtp_host, port, context=context, timeout=15)
    else:
        smtp = smtplib.SMTP(args.smtp_host, port, timeout=15)
    with smtp:
        if args.smtp_security == "starttls":
            smtp.ehlo()
            smtp.starttls(context=context)
            smtp.ehlo()
        smtp.login(args.account, password)
        smtp.send_message(mail, from_addr=args.account, to_addrs=[args.to or args.account])


def tracking_status(api, test_id, headers):
    connection = http.client.HTTPSConnection(api.hostname, api.port or 443, timeout=15,
                                             context=ssl.create_default_context())
    try:
        connection.request("GET", f"/api/deliveries/{test_id}/status", headers=headers)
        response = connection.getresponse()
        body = response.read()
        if response.status == 404:
            return []
        if response.status != 200:
            raise RuntimeError(f"API tracking HTTP {response.status}; periksa URL API dan kredensial App")
        result = json.loads(body)
        if not isinstance(result, list) or any(not isinstance(row, dict) for row in result):
            raise RuntimeError("Respons tracking tidak berupa daftar status")
        return result
    finally:
        connection.close()


def wait_tracking(api, test_id, headers, timeout):
    deadline = time.monotonic() + timeout
    previous = None
    while True:
        rows = tracking_status(api, test_id, headers)
        if rows and rows != previous:
            print(json.dumps(rows, indent=2), flush=True)
            previous = rows
        code, explanation = outcome(rows)
        if code != 2:
            print(explanation)
            return code
        if time.monotonic() >= deadline:
            print(f"{explanation} Cek ulang dengan --status {test_id}")
            return 2
        time.sleep(min(2, max(0, deadline - time.monotonic())))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("account", nargs="?", help="Akun SMTP; penerima default adalah akun ini")
    parser.add_argument("--to", help="Penerima email tes")
    parser.add_argument("--smtp-host", default="mail.moklaar.cloud")
    parser.add_argument("--smtp-port", type=int)
    parser.add_argument("--smtp-security", choices=["tls", "starttls"], default="tls")
    parser.add_argument("--api-url", default="https://mail.moklaar.cloud", help="Origin HTTPS API tracking")
    parser.add_argument("--status", type=external_id, help="Cek ID tes lama tanpa mengirim email lagi")
    parser.add_argument("--wait", type=float, default=60, metavar="SECONDS")
    args = parser.parse_args(argv)
    if not args.status and (not args.account or "@" not in args.account):
        parser.error("isi alamat akun SMTP lengkap untuk mengirim email")
    if not 0 <= args.wait <= 3600:
        parser.error("--wait harus antara 0 dan 3600 detik")
    if args.smtp_port is not None and not 1 <= args.smtp_port <= 65535:
        parser.error("--smtp-port harus antara 1 dan 65535")
    api = urlsplit(args.api_url)
    if (api.scheme != "https" or not api.hostname or api.username or api.password
            or api.path not in ("", "/") or api.query or api.fragment):
        parser.error("--api-url harus berupa origin HTTPS tanpa user/password/path/query")
    headers = {"App-Key": secret("STALWART_APP_KEY", "App-Key"),
               "App-Token": secret("STALWART_APP_TOKEN", "App-Token")}
    test_id = str(args.status or uuid.uuid4())
    print(f"External-ID: {test_id}", flush=True)
    print("Cocokkan externalId ini pada payload yang diterima endpoint webhook.", flush=True)
    if not args.status:
        password = secret("STALWART_SMTP_PASSWORD", "Password SMTP")
        mail = make_message(args.account, args.to or args.account, test_id,
                            headers["App-Key"], headers["App-Token"])
        send_mail(args, password, mail)
        print("DITERIMA SMTP; menunggu hasil pengiriman...", flush=True)
    return wait_tracking(api, test_id, headers, args.wait)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except smtplib.SMTPException as error:
        # Do not echo server text, which may contain submitted credentials.
        code = getattr(error, "smtp_code", None)
        sys.exit(f"SMTP gagal ({type(error).__name__}, kode {code or 'tidak tersedia'})")
    except (OSError, http.client.HTTPException, RuntimeError, ValueError) as error:
        sys.exit(f"Gagal: {error}")
