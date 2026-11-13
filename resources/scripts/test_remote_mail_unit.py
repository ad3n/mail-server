import argparse
import contextlib
import io
import unittest
from unittest.mock import patch

import test_remote_mail as remote


class RemoteMailTests(unittest.TestCase):
    def test_tracking_headers_and_recipient(self):
        mail = remote.make_message("sender@example.test", "recipient@example.test", "test-id", "key", "token")
        self.assertEqual(mail["External-ID"], "test-id")
        self.assertEqual(mail["App-Token"], "token")
        self.assertIsNone(mail["Token-Key"])
        self.assertEqual(mail["To"], "recipient@example.test")
        self.assertNotIn("token", mail.get_content())

    def test_starttls_precedes_login_and_uses_submission_port(self):
        args = argparse.Namespace(smtp_port=None, smtp_host="smtp.example.test",
                                  smtp_security="starttls", account="a@example.test", to=None)
        with patch.object(remote.smtplib, "SMTP") as factory:
            client = factory.return_value
            remote.send_mail(args, "password", remote.make_message(args.account, args.account, "id", "key", "token"))
            factory.assert_called_once_with("smtp.example.test", 587, timeout=15)
            names = [call[0] for call in client.mock_calls if not call[0].startswith("__")]
            self.assertEqual(names, ["ehlo", "starttls", "ehlo", "login", "send_message"])
            context = client.starttls.call_args.kwargs["context"]
            self.assertTrue(context.check_hostname)
            self.assertEqual(context.verify_mode, remote.ssl.CERT_REQUIRED)

    def test_api_does_not_follow_redirect_with_credentials(self):
        with patch.object(remote.http.client, "HTTPSConnection") as factory:
            client = factory.return_value
            client.getresponse.return_value.status = 302
            with self.assertRaisesRegex(RuntimeError, "HTTP 302"):
                remote.tracking_status(remote.urlsplit("https://mail.example.test"), "id", {"App-Token": "secret"})
            self.assertEqual(client.request.call_count, 1)
            client.close.assert_called_once()

    def test_waits_until_delivery_and_reports_failures_or_timeout(self):
        for rows, expected in [([{"status": "accepted"}], 0), ([{"status": "failed"}], 1), ([{"status": "removed"}], 1), ([{"status": "deferred"}], 2), ([{"status": "accepted"}, {"status": "queued"}], 2), ([], 2)]:
            with self.subTest(rows=rows), patch.object(remote, "tracking_status", return_value=rows), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(remote.wait_tracking(None, "id", {}, 0), expected)
        with patch.object(remote, "tracking_status", side_effect=[[{"status": "queued"}], [{"status": "accepted"}]]), patch.object(remote.time, "sleep"), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(remote.wait_tracking(None, "id", {}, 10), 0)

    def test_status_never_sends_or_requests_smtp_password(self):
        test_id = "c6e96f18347f4c69e4064668e1d6d1fb"
        with patch.object(remote, "secret", return_value="test-secret") as secret, patch.object(remote, "send_mail") as send, patch.object(remote, "wait_tracking", return_value=0), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(remote.main(["--status", test_id]), 0)
            send.assert_not_called()
            self.assertEqual([call.args[0] for call in secret.call_args_list], ["STALWART_APP_KEY", "STALWART_APP_TOKEN"])


if __name__ == "__main__":
    unittest.main()
