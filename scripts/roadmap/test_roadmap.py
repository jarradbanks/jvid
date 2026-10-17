"""Tests for the roadmap Status column. No network: gh is never called for real.

usage: python3 scripts/roadmap/test_roadmap.py
"""
import pathlib
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import roadmap  # noqa: E402


def issue(number, state="OPEN", labels=()):
    return {"number": number, "state": state, "labels": [{"name": name} for name in labels]}


def pr(*closes):
    return {"closingIssuesReferences": [{"number": n} for n in closes]}


class StatusesTest(unittest.TestCase):
    def test_closed_is_done(self):
        self.assertEqual(roadmap.statuses([issue(1, "CLOSED")], []), {1: "done"})

    def test_need_testing_label(self):
        self.assertEqual(roadmap.statuses([issue(2, labels=["area:infra", "need testing"])], []),
                         {2: "need testing"})

    def test_open_pr_closing_reference_is_in_progress(self):
        self.assertEqual(roadmap.statuses([issue(3), issue(4), issue(5)], [pr(3), pr(4, 5)]),
                         {3: "in progress", 4: "in progress", 5: "in progress"})

    def test_body_mentions_are_ignored(self):
        prs = [{"body": "Closes #6. Part of #7.", "closingIssuesReferences": []}]
        self.assertEqual(roadmap.statuses([issue(6), issue(7)], prs), {})

    def test_untouched_issue_has_no_status(self):
        self.assertEqual(roadmap.statuses([issue(8), issue(12)], [pr(1), pr(123)]), {})

    def test_precedence_done_then_need_testing_then_in_progress(self):
        issues = [issue(9, "CLOSED", ["need testing"]), issue(10, labels=["need testing"])]
        self.assertEqual(roadmap.statuses(issues, [pr(9, 10)]), {9: "done", 10: "need testing"})

    def test_missing_pr_references(self):
        self.assertEqual(roadmap.statuses([issue(11)], [{"closingIssuesReferences": None}, {}]), {})


class FetchStatusesTest(unittest.TestCase):
    def check_blank_with_warning(self, error):
        with mock.patch.object(roadmap.subprocess, "run", side_effect=error), \
                mock.patch.object(sys, "stderr") as stderr:
            self.assertEqual(roadmap.fetch_statuses(), {})
        self.assertIn("warning", "".join(c.args[0] for c in stderr.write.call_args_list))

    def test_gh_missing(self):
        self.check_blank_with_warning(FileNotFoundError(2, "No such file or directory", "gh"))

    def test_gh_fails(self):
        self.check_blank_with_warning(subprocess.CalledProcessError(1, ["gh"], stderr="HTTP 401: Bad credentials"))

    def test_uses_both_gh_calls(self):
        outputs = ['[{"number": 1, "state": "OPEN", "labels": []}]', '[{"closingIssuesReferences": [{"number": 1}]}]']
        runs = [mock.Mock(stdout=out) for out in outputs]
        with mock.patch.object(roadmap.subprocess, "run", side_effect=runs) as run:
            self.assertEqual(roadmap.fetch_statuses(), {1: "in progress"})
        self.assertEqual([c.args[0][:3] for c in run.call_args_list],
                         [["gh", "issue", "list"], ["gh", "pr", "list"]])


if __name__ == "__main__":
    unittest.main()
