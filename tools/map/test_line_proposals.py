"""Synthetic pattern checks and anti-forgery checks; no map accuracy claims."""
from types import SimpleNamespace
import tempfile
import unittest
from pathlib import Path
from propose_lines import scan, select_audit, validate_output
from audit_lines import report

class SyntheticImage:
    width=160; height=160
    def __init__(self, feature): self.feature=feature
    def getpixel(self, pos): return self.feature(*pos)

A=SimpleNamespace(x=40,y=80)
B=SimpleNamespace(x=120,y=80)
CLEAR=(251,250,239)
BROWN=(72,63,34)

class ProposalTests(unittest.TestCase):
    def test_offset_double_stroke_crossing_is_road(self):
        image=SyntheticImage(lambda x,y:BROWN if y in (81,85) else CLEAR)
        result={r['kind']:r for r in scan(image,A,B)}
        self.assertEqual(result['road']['proposal'],'present')
        self.assertEqual(result['unfinished_road']['proposal'],'uncertain')

    def test_parallel_grid_cannot_be_a_crossing_line(self):
        image=SyntheticImage(lambda x,y:(0,0,0) if x==80 else CLEAR)
        for row in scan(image,A,B): self.assertNotEqual(row['proposal'],'present')

    def test_dashed_double_stroke_does_not_certify_completed_road(self):
        image=SyntheticImage(lambda x,y:BROWN if y in (81,85) and x%14<7 else CLEAR)
        result={r['kind']:r for r in scan(image,A,B)}
        self.assertNotEqual(result['road']['proposal'],'present')
        self.assertEqual(result['unfinished_road']['proposal'],'uncertain')

    def test_clipped_image_never_certifies_absence(self):
        image=SyntheticImage(lambda x,y:CLEAR)
        a=SimpleNamespace(x=1,y=1);b=SimpleNamespace(x=30,y=1)
        self.assertTrue(all(r['proposal']=='uncertain' for r in scan(image,a,b)))

    def test_output_cannot_overwrite_audit_or_enter_repo(self):
        with tempfile.TemporaryDirectory() as root:
            folder=Path(root);sources=folder/'sources';sources.mkdir()
            with self.assertRaises(ValueError): validate_output(sources,sources/'child')
            with self.assertRaises(ValueError): validate_output(sources,Path(__file__).parent/'local-art')
            output=folder/'output';output.mkdir();(output/'audit.csv').write_text('review')
            with self.assertRaises(ValueError): validate_output(sources,output)

class AuditTests(unittest.TestCase):
    def fixture(self):
        rows=[dict(from_hex='C4218',to_hex='C4219',kind='track',proposal='present',confidence='0.9',score='0.8')]
        meta=dict(seed=6022,sample_per_kind_state=8,populations={'track/present':1},algorithm='synthetic',
            build_file_sha256='x',source_image_sha256='y',selection={})
        audit=[dict(rows[0],observed='unresolved',note='obscured')]
        return rows,audit,meta

    def test_unresolved_is_never_zero_error_accuracy(self):
        rows,audit,meta=self.fixture();data=report(rows,audit,meta)['layers']['track']
        self.assertIsNone(data['conditional_sample_error'])
        self.assertEqual(data['strata']['present']['unresolved'],1)

    def test_false_positive_counted_and_abstention_not_counted_as_error(self):
        rows,audit,meta=self.fixture();audit[0]['observed']='absent'
        data=report(rows,audit,meta)['layers']['track']
        self.assertEqual(data['confident_errors'],1)
        self.assertEqual(data['conditional_sample_error'],1)

    def test_prediction_tampering_and_duplicates_rejected(self):
        rows,audit,meta=self.fixture();audit[0]['score']='0.7'
        with self.assertRaisesRegex(ValueError,'changed'): report(rows,audit,meta)
        rows,audit,meta=self.fixture()
        with self.assertRaisesRegex(ValueError,'Duplicate audit'): report(rows,audit+audit,meta)

    def test_missing_labels_and_nonseeded_selection_rejected(self):
        rows,audit,meta=self.fixture();audit[0]['observed']=''
        with self.assertRaisesRegex(ValueError,'observed'): report(rows,audit,meta)
        with self.assertRaisesRegex(ValueError,'cohort'): report(rows,[],meta)

    def test_seed_sample_is_stable_under_input_order(self):
        rows=[dict(from_hex=f'C{n:04}',to_hex=f'C{n+1:04}',kind='road',proposal='absent') for n in range(30)]
        self.assertEqual(select_audit(rows,91,8),select_audit(list(reversed(rows)),91,8))
        self.assertNotEqual(select_audit(rows,91,8),select_audit(rows,92,8))

if __name__=='__main__': unittest.main()
