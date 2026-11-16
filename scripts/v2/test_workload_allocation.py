import unittest

from workload_allocation import allocate


class AllocationTests(unittest.TestCase):
    def test_pairs_and_all_three(self):
        for counts, expected in [
            ([5, 0, 5], (10, 5, 3, 0, 2)),
            ([5, 5, 0], (10, 5, 3, 2, 0)),
            ([0, 5, 5], (10, 5, 0, 3, 2)),
            ([3, 3, 4], (10, 5, 2, 1, 2)),
            ([0, 10, 0], (10, 5, 0, 5, 0)),
        ]:
            with self.subTest(counts=counts):
                self.assertEqual(allocate(counts, "50"), expected)

    def test_boundaries(self):
        self.assertEqual(allocate([1, 1, 1], "0"), (3, 0, 0, 0, 0))
        self.assertEqual(allocate([1, 1, 1], "0.01"), (3, 1, 1, 0, 0))
        self.assertEqual(allocate([1, 1, 1], "100"), (3, 3, 1, 1, 1))
        self.assertEqual(allocate([0, 1, 0], "33.3"), (1, 1, 0, 1, 0))

    def test_invalid(self):
        for counts, percentage in [([0, 0, 0], "50"), ([-1, 1, 1], "50"),
                                   ([1, 1, 1], "-1"), ([1, 1, 1], "100.1"),
                                   ([1, 1, 1], "NaN"), ([1, 1, 1], "bad")]:
            with self.subTest(counts=counts, percentage=percentage):
                with self.assertRaises(ValueError):
                    allocate(counts, percentage)


if __name__ == "__main__":
    unittest.main()