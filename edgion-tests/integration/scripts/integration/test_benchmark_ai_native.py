#!/usr/bin/env python3
"""Pure measurement invariants; no networking or live processes."""
import hashlib
import unittest
import benchmark_ai_native as bench


class MeasurementTests(unittest.TestCase):
    def test_percentile_interpolates_and_rejects_missing_measurements(self):
        self.assertEqual(bench.percentile([9, 1, 5, 3], .5), 4)
        self.assertAlmostEqual(bench.percentile([10, 20], .95), 19.5)
        for values in [[], [float('nan')], [-1]]:
            with self.assertRaises(bench.MeasurementFailure):
                bench.distribution(values)

    def test_cpu_time_supports_portable_ps_shapes_and_resolution(self):
        self.assertEqual(bench.cpu_seconds('01:02.34'), (62.34, .01))
        self.assertEqual(bench.cpu_seconds('02:03:04'), (7384.0, 1.0))
        self.assertEqual(bench.cpu_seconds('1-02:03:04'), (93784.0, 1.0))
        self.assertEqual(bench.validate_sample_line('42 100 00:02.00'),
                         (42, {'rss_kib': 100, 'cpu_seconds': 2.0, 'cpu_resolution_seconds': .01}))
        with self.assertRaises(bench.MeasurementFailure):
            bench.validate_sample_line('42 -1 00:00')

    def test_stream_count_digest_include_terminal_bytes(self):
        chunks = [bench.frame(4)] * 3 + [bench.TERMINAL]
        wire = b''.join(chunks)
        self.assertEqual(bench.expected(True, 4, 3), (len(wire), hashlib.sha256(wire).hexdigest()))
        self.assertTrue(wire.endswith(b'data: [DONE]\n\n'))

    def test_independent_size_count_and_wire_limits(self):
        for payload in [0, bench.FRAME_PAYLOAD + 1]:
            with self.assertRaises(bench.MeasurementFailure):
                bench.frame(payload)
        for count in [0, 65536, 2 ** 40]:
            with self.assertRaises(bench.MeasurementFailure):
                bench.expected(True, bench.FRAME_PAYLOAD, count)
        # Largest planned cancellation workload remains under the fixture wire cap.
        self.assertLess(len(bench.frame(bench.FRAME_PAYLOAD)) * 60000 + len(bench.TERMINAL), 1024 ** 3)

    def test_lifecycle_markers_are_absent_from_timed_traffic(self):
        self.assertNotIn('access_log', bench.headers('provider'))
        self.assertNotIn('x-ai-performance-observe', bench.headers('off'))
        marker = 'a' * 32
        self.assertEqual(bench.headers('off', marker)['traceparent'], '00-' + marker + '-0123456789abcdef-00')
        self.assertEqual(bench.headers('off')['Accept-Encoding'], 'identity')


if __name__ == '__main__':
    unittest.main()
