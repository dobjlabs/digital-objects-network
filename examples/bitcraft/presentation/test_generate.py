"""Tests for reading the hand-written copy in generate.py.

    python3 examples/bitcraft/presentation/test_generate.py
"""

import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

import generate


def read_copy(text):
    """A Copy of `text`, and the list its errors go to."""
    folder = tempfile.TemporaryDirectory()
    path = Path(folder.name) / "actions.md"
    path.write_text(textwrap.dedent(text))
    errors = []
    copy = generate.Copy(path, errors)
    folder.cleanup()
    return copy, errors


class CopyTest(unittest.TestCase):
    def test_summary_and_description_paragraphs(self):
        copy, errors = read_copy("""\
            # Title and notes before the first entry are ignored.

            ## GatherWood
            Gather 1 wood
            with your pocketknife.

            First paragraph.

            Second paragraph.

            ## CraftStick
            Split 1 wood into 2 sticks.
            """)
        self.assertEqual(copy.text_for("GatherWood"), (
            "Gather 1 wood with your pocketknife.", "First paragraph.\n\nSecond paragraph."))
        self.assertEqual(copy.text_for("CraftStick"), ("Split 1 wood into 2 sticks.", None))
        self.assertEqual(errors, [])

    def test_family_fills_in_placeholders_filters_and_arithmetic(self):
        copy, errors = read_copy("""\
            ## LevelUpQuarry{level}
            Raise a level {level} quarry to level {level + 1}.

            Spends {20 * 2 ** level} wood. {{Braces}} stay.

            ## Rekey{class}
            Give a {class|words} a fresh key.

            ## BucketOf{resource}Add{amount}
            Put {amount} {resource|lower} into a bucket.
            """)
        self.assertEqual(copy.text_for("LevelUpQuarry3"), (
            "Raise a level 3 quarry to level 4.", "Spends 160 wood. {Braces} stay."))
        self.assertEqual(copy.text_for("RekeyBucketOfWood"),
                         ("Give a bucket of wood a fresh key.", None))
        self.assertEqual(copy.text_for("BucketOfStoneAdd10"),
                         ("Put 10 stone into a bucket.", None))
        self.assertIsNone(copy.text_for("CraftStick"))
        self.assertEqual(errors, [])

    def test_an_exact_entry_overrides_its_family(self):
        copy, errors = read_copy("""\
            ## UseQuarry{level}
            Mine {level + 1} stone.

            ## UseQuarry6
            Mine 7 stone with the best quarry there is.
            """)
        self.assertEqual(copy.text_for("UseQuarry2"), ("Mine 3 stone.", None))
        self.assertEqual(copy.text_for("UseQuarry6"),
                         ("Mine 7 stone with the best quarry there is.", None))
        self.assertEqual(errors, [])

    def test_a_name_two_families_fit_is_an_error(self):
        copy, errors = read_copy("""\
            ## Use{tool}
            Use it.

            ## UseQuarry{level}
            Mine.
            """)
        self.assertIsNone(copy.text_for("UseQuarry2"))
        self.assertEqual(len(errors), 1)
        self.assertIn("'UseQuarry2' fits more than one family", errors[0])

    def test_entries_that_match_nothing_are_reported(self):
        copy, errors = read_copy("""\
            ## GatherWood
            Gather.

            ## GatherGold
            Gather gold.

            ## UseDrill{level}
            Drill.
            """)
        copy.text_for("GatherWood")
        copy.report_unused()
        self.assertEqual(errors, [
            "copy/actions.md:4: 'GatherGold' matches nothing in ../manifest.toml",
            "copy/actions.md:7: 'UseDrill{level}' matches nothing in ../manifest.toml",
        ])

    def test_malformed_entries_are_reported(self):
        copy, errors = read_copy("""\
            ## GatherWood

            ## Gather Wood
            Two words.

            ## CraftStick
            Split.

            ## CraftStick
            Again.

            ### Notes
            """)
        self.assertEqual(errors, [
            "copy/actions.md:1: 'GatherWood' has no summary",
            "copy/actions.md:3: 'Gather Wood' is not a name or a family like UseQuarry{level}",
            "copy/actions.md:12: only '## Name' headings are allowed after the first entry",
            "copy/actions.md:9: 'CraftStick' already has an entry, at line 6",
        ])

    def test_bad_fields_are_reported_and_left_as_written(self):
        copy, errors = read_copy("""\
            ## UseQuarry{level}
            {tool} {level|upper} {level * name} {level / 2} {level + 1}
            """)
        self.assertEqual(copy.text_for("UseQuarry2"),
                         ("{tool} {level|upper} {level * name} {level / 2} 3", None))
        self.assertEqual(errors, [
            "copy/actions.md:1: {tool}: no placeholder 'tool' in the heading",
            "copy/actions.md:1: {level|upper}: no filter 'upper'; use lower or words",
            "copy/actions.md:1: {level * name}: no placeholder 'name' in the heading",
            "copy/actions.md:1: {level / 2}: only placeholders, whole numbers and "
            "+ - * ** // are allowed",
        ])

    def test_arithmetic_needs_a_number(self):
        copy, errors = read_copy("""\
            ## Gather{resource}
            {resource + 1}
            """)
        copy.text_for("GatherWood")
        self.assertEqual(errors, [
            "copy/actions.md:1: {resource + 1}: 'resource' is 'Wood', not a whole number"])


class PackTest(unittest.TestCase):
    def test_the_committed_pack_matches_the_copy(self):
        result = subprocess.run([sys.executable, str(generate.PACK / "generate.py"), "--check"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
