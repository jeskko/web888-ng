# Bundled marker and band-plan data

What `web888-markers` compiles in, where each file comes from, and on what
terms. Nothing here is fetched at runtime.

Except for EiBi's file, the files are our own compilation of facts
(frequencies, band edges, station names, modes), taken from the primary
sources listed with each one: the operators', authorities' and IARU's own
publications. No third-party frequency list was used, not even as a
cross-check. The short notes are written in our own words. These files are
part of web888-ng and under its licence, GPL-3.0-or-later.

## eibi-b26.csv: shortwave schedules (season B26)

EiBi's CSV database for the winter season B26 (October 2026 - March 2027),
by Eike Bierwirth, <http://www.eibispace.de/> (`dx/sked-b26.csv`, fetched
2026-11-04; SHA-256 `f877bbd75e3c04dcd29dddfe2c4a08fe351e8c9b5a6eabf654d20c78e66f1bcb`). Unmodified: ISO 8859-1, CRLF line ends.

Terms, from EiBi's README.TXT, "A) Conditions of use": *"All my frequency
lists are free of cost, and any person is absolutely free to download, use,
copy, or distribute these files or to use them within third-party
software."* Corrections go to EiBi (see that README).

A new season: put the new `sked-<season>.csv` here in place of this file
and update `CSV`, `SEASON` and `WINTER` in `src/eibi.rs` (A seasons start
at the end of March, B seasons at the end of October).

## utility.json: utility stations (157 markers)

Time signals, maritime distress and safety (DSC, NAVTEX, NBDP, voice),
aeronautical (VOLMET, North Atlantic air-ground, SAR), weather fax, DWD
RTTY, the NCDXF/IARU beacons.

* `hz` is the published frequency. Fax entries give `dial` 1.9 kHz below the
  published (centre) figure, as for USB reception. DSC, NAVTEX, NBDP and
  RTTY entries give `dial` 1.7 kHz below, which puts the tones at 1700 Hz
  audio (ITU-R M.493). Those dials are our choice, not published figures.
* `emission` and `params` (baud, shift, LPM, IOC) are as published.
* Left out: stations whose only published data is old (some fax stations
  with information from 2009-2019), and ones no operator publication could
  be found for (USAF HFGCS; the New York, Tokyo, Hong Kong and Sydney
  VOLMETs). Gander VOLMET (closed June 2025) and stopped time and fax
  stations are not listed.

Sources:
  * BIPM Annual Report on Time Activities 2025: Time signals (tables based on information received from the responsible authorities, Feb-Mar 2026): <https://webtai.bipm.org/ftp/pub/tai/annual-reports/bipm-annual-report/TIMESIGNALS/timesignals_2025.pdf>
  * NIST: Radio Station WWV: <https://www.nist.gov/pml/time-and-frequency-division/time-distribution/radio-station-wwv>
  * ITU Radio Regulations (2024), Appendix 15 (Rev.WRC-23), Table 15-1: GMDSS distress and safety frequencies below 30 MHz: <https://www.itu.int/pub/R-REG-RR-2024>
  * Recommendation ITU-R M.493-16: Digital selective-calling system for use in the maritime mobile service: <https://www.itu.int/rec/R-REC-M.493/en>
  * Recommendation ITU-R M.625-4: Direct-printing telegraph equipment employing automatic identification in the maritime mobile service: <https://www.itu.int/rec/R-REC-M.625/en>
  * NOAA/NWS: Worldwide Marine Radiofacsimile Broadcast Schedules (March 7, 2025): <https://www.weather.gov/media/marine/rfax.pdf>
  * Deutscher Wetterdienst: Facsimile transmission for shipping (09/2023): <https://www.dwd.de/EN/specialusers/shipping/broadcast_en/broadcast_fax_092023.pdf?__blob=publicationFile>
  * Deutscher Wetterdienst: RTTY broadcast for shipping, programmes 1 and 2 (09/2023): <https://www.dwd.de/EN/specialusers/shipping/broadcast_en/_node.html>
  * NCDXF/IARU International Beacon Project: <https://www.ncdxf.org/beacon/>
  * AirNav Ireland: North Atlantic Communications frequencies: <https://airnav.ie/what-we-do/north-atlantic-communications/frequencies>
  * AIP Thailand GEN 3.5, Table GEN 3.5.7 VOLMET service (CAAT, AIRAC 2024-02-22): <https://aip.caat.or.th/2024-02-22-AIRAC/pdf/VT-GEN-3.5-en-GB.pdf>

## amateur.json: amateur activity frequencies (125 markers)

Digital-mode dials (FT8, FT4, WSPR, JT65, JT9, FST4, FST4W, JS8), calling,
QRP, SSTV, AM and emergency centres of activity, and the NCDXF beacons,
2200 m to 6 m, each with the IARU regions it applies to.

* WSJT-X's and JS8Call's default frequency tables (GPL-3.0) were read for
  the numbers only; no code was taken.
* Where the plans disagree (e.g. 160 m CW QRP 1810 or 1812 kHz), both are
  listed, each with its source.

Sources:
  * WSJT-X models/FrequencyList.cpp (default frequency list), wsjtx-2.7.0 (d2d9c46a; identical at master b4f9a431): <https://sourceforge.net/p/wsjt/wsjtx/ci/wsjtx-2.7.0/tree/models/FrequencyList.cpp>
  * JS8Call FrequencyList.cpp (default frequency list), v2.3.1 (fd721e8b; identical at HEAD a7ff1be0): <https://github.com/js8call/js8call/blob/v2.3.1/FrequencyList.cpp>
  * IARU Region 1 HF Band Plan, effective 16 Oct 2020 (PDF of June 2021): <https://www.iaru-r1.org/wp-content/uploads/2021/06/hf_r1_bandplan.pdf>
  * IARU Region 1 VHF Handbook, 50-54 MHz band plan, 10.03, February 2026: <https://www.iaru-r1.org/wp-content/uploads/2026/02/VHF_Handbook_V10_03_final.pdf>
  * IARU Region 2 Band Plan, September 2020: <https://www.iaru-r2.org/wp-content/uploads/2020/02/IARU-Region-2-Band-plan.pdf>
  * IARU Region 3 Band Plans, Document R3-004, Rev. 2, November 2024: <https://iaru-r3.org/wp-content/uploads/2025/01/R3-004-Band-Plans-IARU-Region-3.pdf>
  * ARRL Band Plan, retrieved 2026-11-04: <https://www.arrl.org/band-plan>

## iaru-r1.json, iaru-r2.json, iaru-r3.json: amateur band plans

Each IARU region's band plan, 2200 m to 6 m: the bands, their segments (CW,
narrow-band digital, all modes, beacons, satellite, FM) and the centres of
activity the plans name.

* Where a plan's own rows overlap, they are split so that the segments
  tile, and the note says so. Entries marked `"verify": true` are readings
  of an unclear plan (noted).

Sources, region 1:
  * IARU Region 1 HF Band Plan, effective 16 October 2020 (Novi Sad), file dated June 2021: <https://www.iaru-r1.org/wp-content/uploads/2021/06/hf_r1_bandplan.pdf>
  * IARU Region 1 VHF Handbook (50-54 MHz band plan), 10.03, February 2026: <https://www.iaru-r1.org/wp-content/uploads/2026/02/VHF_Handbook_V10_03_final.pdf>

Region 2:
  * IARU Region 2 Band Plan, September 2020: <https://www.iaru-r2.org/wp-content/uploads/2020/02/IARU-Region-2-Band-plan.pdf>

Region 3:
  * IARU Region 3 Band Plan (Document R3-004), Rev. 2, November 2024 (19th Region 3 Conference, Bangkok): <https://www.iaru-r3.org/wp-content/uploads/2025/01/R3-004-Band-Plans-IARU-Region-3.pdf>

## itu-services.json: other services' bands (116 entries)

Broadcasting (LW, MW, tropical, international HF, the low VHF band),
maritime mobile (with the Appendix 17 sub-bands, marked `"sub": true`),
aeronautical mobile (R), standard frequency and time signals, ISM, and CB
(national: the CEPT and US channel plans).

* Per ITU region, from the Radio Regulations, Edition of 2024: Article 5,
  Article 23 (No. 23.6, the tropical zone) and Appendix 17.
* The LW carrier grid in its note is derived (`"verify": true`).

Sources:
  * ITU Radio Regulations, Vol. 1 Art. 5 (Table of Frequency Allocations) and Art. 23; Vol. 2 Appendix 17, Edition of 2024 (WRC-23): <https://www.itu.int/pub/R-REG-RR-2024>
  * Final Acts of the Regional Administrative LF/MF Broadcasting Conference (Regions 1 and 3), Geneva, 1975, ITU, 1976: <https://www.itu.int/pub/R-ACT-RRC.3-1975>
  * FCC Online Table of Frequency Allocations, 47 CFR 2.106, revised July 1, 2022: <https://transition.fcc.gov/oet/spectrum/table/fcctable.pdf>
  * 47 CFR 73.14 (AM broadcast definitions) and 95.963 (CB channels), eCFR, retrieved 2026-11-04: <https://www.ecfr.gov/current/title-47>
  * ECC Decision (11)03: harmonised use of frequencies for CB radio equipment, approved 24 June 2011, amended 17 June 2016: <https://docdb.cept.org/document/430>
