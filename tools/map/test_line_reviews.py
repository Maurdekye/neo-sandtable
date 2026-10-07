"""Source-bound review replay must not certify unaudited or unresolved edges."""
import csv
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from geometry import Grid
from layers import Layers, UnknownCoverage
from line_reviews import load_line_reviews

MAP=Path(__file__).resolve().parents[2]/'data/map'

class LineReviewTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.folder=Path(self.temp.name)
        shutil.copytree(MAP/'line-reviews',self.folder/'reviews')
        self.grid=Grid(MAP)
        self.path=self.folder/'reviews/validation-0001/metadata.json'
        self.meta=json.loads(self.path.read_text())
        self.image_hash=self.meta['source_image_sha256'];self.build_hash=self.meta['build_file_sha256']

    def load(self): return load_line_reviews(self.folder/'reviews',self.grid,self.image_hash,self.build_hash)
    def save(self): self.path.write_text(json.dumps(self.meta))

    def test_only_resolved_audit_rows_become_masks(self):
        features,masks=self.load()
        self.assertEqual(len(masks),55)
        self.assertEqual(len(features),9)
        self.assertNotIn(('line:track','C4320','C4419'),[(r['layer'],r['hex_id'],r['neighbour_id']) for r in masks])
        self.assertTrue(any(r['from_hex']=='C4320' and r['to_hex']=='C4419' and r['kind']=='road' for r in features))

    def test_review_hash_and_source_changes_fail(self):
        with self.assertRaisesRegex(ValueError,'source identity'):
            load_line_reviews(self.folder/'reviews',self.grid,'new-image',self.build_hash)
        audit=self.path.parent/'audit.csv';audit.write_text(audit.read_text()+'\n')
        with self.assertRaisesRegex(ValueError,'hash changed'): self.load()

    def test_windows_checkout_line_endings_preserve_review_identity(self):
        for name in ("proposals.csv","audit.csv"):
            path=self.path.parent/name
            path.write_bytes(path.read_bytes().replace(b"\n",b"\r\n"))
        self.assertEqual(len(self.load()[1]),55)

    def test_no_implicit_acceptance_of_proposal_bundle(self):
        self.meta['publish_reviewed_labels']=False;self.save()
        with self.assertRaisesRegex(ValueError,'explicitly accepted'): self.load()

    def test_duplicate_review_batches_cannot_override(self):
        other=self.folder/'reviews/duplicate';shutil.copytree(self.path.parent,other)
        meta=json.loads((other/'metadata.json').read_text());meta['review_id']='duplicate'
        (other/'metadata.json').write_text(json.dumps(meta))
        with self.assertRaisesRegex(ValueError,'Duplicate surveyed'): self.load()

    def test_replay_equals_public_files_and_kind_queries(self):
        features,masks=self.load()
        with (MAP/'line_features.csv').open(newline='') as f: self.assertEqual(features,[r for r in csv.DictReader(f) if r["review_batch"]==self.meta["review_id"]])
        with (MAP/'coverage.csv').open(newline='') as f:
            actual=[r for r in csv.DictReader(f) if r['layer'].startswith('line:') and r['review_batch']==self.meta['review_id']]
        self.assertEqual(masks,actual)
        data=Layers(MAP)
        self.assertIsNotNone(data.feature('line','road','C4419','C4320'))
        with self.assertRaises(UnknownCoverage): data.feature('line','track','C4419','C4320')
        self.assertIsNone(data.feature('line','road','C4416','C4417'))
        with self.assertRaises(UnknownCoverage): data.feature('line','pipeline','C4416','C4417')

if __name__=='__main__': unittest.main()
