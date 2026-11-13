import contextlib
import io
import json
import unittest
from unittest.mock import patch

import mail_tracking
import test_local_mail as local


class TrackingTests(unittest.TestCase):
    def test_stalwart_statuses_and_multiple_recipients(self):
        cases = [([], 2), (["queued"], 2), (["deferred"], 2),
                 (["accepted"], 0), (["accepted", "accepted"], 0),
                 (["accepted", "deferred"], 2), (["failed"], 1),
                 (["removed"], 1), (["accepted", "failed"], 1),
                 (["queued", "removed"], 1)]
        for states, expected in cases:
            with self.subTest(states=states):
                rows = [{"recipient": f"user{i}@example.test", "status": status}
                        for i, status in enumerate(states)]
                self.assertEqual(mail_tracking.outcome(rows)[0], expected)

    def test_invalid_responses_are_not_success(self):
        for rows in [None, {}, [None], [{}], [{"status": []}], [{"status": "delivered"}]]:
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                mail_tracking.outcome(rows)

    def test_external_ids_match_server_rules(self):
        for value in ["c6e96f18347f4c69e4064668e1d6d1fb", "order_42.test-1", "a" * 128]:
            self.assertEqual(mail_tracking.external_id(value), value)
        for value in ["", ".", "..", "x/y", "a" * 129, "x\r\ny", "é"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                mail_tracking.external_id(value)

    def test_local_http_200_uses_delivery_status(self):
        for states, expected in [(["accepted"], 0), (["failed"], 1), (["removed"], 1),
                                 (["queued"], 2), (["deferred"], 2), ([], 2)]:
            with self.subTest(states=states), patch.object(local.http.client, "HTTPConnection") as factory, contextlib.redirect_stdout(io.StringIO()):
                client = factory.return_value
                response = client.getresponse.return_value
                response.status = 200
                response.read.return_value = json.dumps([{"status": state} for state in states]).encode()
                self.assertEqual(local.show_tracking("test-id", {}), expected)
                client.close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
