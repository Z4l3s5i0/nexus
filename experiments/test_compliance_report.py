import importlib.util
import pathlib
import tempfile
import unittest


REPORT_PATH = pathlib.Path(__file__).resolve().parent / "compliance" / "report.py"
SPEC = importlib.util.spec_from_file_location("compliance_report", REPORT_PATH)
compliance_report = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(compliance_report)


class ComplianceReportTests(unittest.TestCase):
    def test_results_reader_rejects_malformed_jsonl(self):
        with tempfile.TemporaryDirectory() as temp:
            path = pathlib.Path(temp) / "results.jsonl"
            path.write_text('{"client":"wasix-r-eth"}\nnot-json\n', encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "not valid JSON"):
                compliance_report.read_results(path)

    def test_results_reader_rejects_missing_results_file(self):
        with tempfile.TemporaryDirectory() as temp:
            path = pathlib.Path(temp) / "results.jsonl"

            with self.assertRaisesRegex(FileNotFoundError, "required compliance results"):
                compliance_report.read_results(path)


if __name__ == "__main__":
    unittest.main()