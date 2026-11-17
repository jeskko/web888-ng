# hfcast and the VOACAP data in web888d

`web888d` includes **hfcast** (`crates/prop`), a Rust translation of
VOACAP by Jonathan McSweet, used under the Apache License 2.0
(<https://github.com/jonathanmcsweet/hfcast-engine>; we build from our
fork, <https://github.com/jeskko/hfcast-engine>). Its `NOTICE`:

> HFcast Engine. Copyright 2026 Jonathan McSweet.
>
> This product includes software translated from VOACAP, the Voice of
> America Coverage Analysis Program. VOACAP was developed by the
> Institute for Telecommunication Sciences (ITS), National
> Telecommunications and Information Administration (NTIA), United States
> Department of Commerce, and is maintained by Greg Hand. As a work of
> the United States Government it is stated to be not subject to
> copyright protection in the United States. The Unix build of VOACAP,
> `voacapl`, and the changes it carries, are by J.A. Watson, released
> under CC0 1.0 Universal.

NTIA/ITS disclaimer, carried forward from the original distribution:

> The NTIA/ITS makes no warranty, express or implied, as to the
> accuracy, completeness or usefulness of this software, and assumes no
> liability for its use. Use of this software does not imply endorsement
> by NTIA/ITS or the United States Government.

## The coefficient data compiled into web888d

The GPL does not cover these data; they keep their own status.

* About 195 KB (sporadic-E, E and F1 region maps, the F2 height ratio,
  the solar index table, prediction error statistics), the antenna
  pattern files and the version file: NTIA/ITS work, as VOACAP above.
* About 216 KB of atmospheric radio noise (Fam, its variability and
  related maps), from CCIR Report 322, and about 134 KB of foF2 and
  M(3000)F2 ionospheric maps, from CCIR Report 340. The ITU publishes
  these coefficients itself in the reference software of ITU-R Study
  Group 3 for Recommendations P.372 and P.533
  (<https://github.com/ITU-R-Study-Group-3/ITU-R-HF>), with this
  statement:

  > The ITURHFProp, P533 and P372 software has been developed
  > collaboratively by participants in ITU-R Study Group 3. It may be
  > used by implementers in their implementation of the Recommendation
  > as well as in revisions of the specific original Recommendation and
  > in other ITU Recommendations, free from any copyright assertions.

  The files here come through the VOACAP distribution, not the ITU's
  release; the statement is worded for implementations of ITU
  Recommendations, which VOACAP is not.
* The URSI-88 foF2 maps are not included.

Thanks to the Voice of America for VOACAP, NTIA/ITS for IONCAP and the
data, Greg Hand for maintaining VOACAP, and J.A. Watson for voacapl.
