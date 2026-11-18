# Glossary

The technical terms of the web UI, what each means here, and how each
language says it. Translators: use these, so the same thing has the same
name everywhere; if you change one, change it here and in your `.po`.
"—" keeps the English (or international) term.

Kept as they are in every language: mode names (USB, LSB, CW, AM, SAM,
FM, IQ, FT8, FT4, WSPR, JS8, RTTY, NAVTEX, SITOR-B, DSC, PSK31, HFDL,
SSTV, FAX), units, UTC, and AGC, MUF, LUF, SNR, ADC, PGA, OVL, ATT
where the UI shows them as abbreviations.

## Short button labels

The header is narrow; these are the choices so far (Suomi / Svenska / Deutsch): Config → Asetukset / Inställn. / Einst.; Rec → Äänitä / Rec / Aufn.; Mem → Muisti / Minne / Speicher; Vol → Ääni / Vol / Lautst.; Prop → Kelit / Prognos / Prognose; Notch → Notch / Notch / Notch

## Receiving

| English | meaning here | Suomi | Svenska | Deutsch |
|---|---|---|---|---|
| receiver | the Web-888 (or one of its receive channels) | vastaanotin | mottagare | Empfänger |
| mode | the kind of modulation or transmission (USB, CW, FT8...) | lähetelaji | mod (pl. moder) | Betriebsart ("Mode" in short column heads) |
| user | a person connected to the receiver | käyttäjä | användare | Nutzer |
| listener | someone using the page | kuuntelija | lyssnare | Hörer |
| dial, dial frequency | the frequency tuned to | viritystaajuus | inställd frekvens | Abstimmfrequenz |
| tune | set the dial | virittää | ställa in (not "stämma av", which reads as reconcile) | abstimmen |
| step | how far one click moves the dial | askel | steg | Schrittweite |
| passband | the audio filter's range around the dial | päästökaista | passband | Durchlassbereich |
| bandwidth | the passband's width | kaistanleveys | bandbredd | Bandbreite |
| sideband | upper/lower | sivunauha (ylä-, ala-) | sidband (övre, undre) | Seitenband (oberes, unteres) |
| carrier | an unmodulated signal or AM's carrier | kantoaalto | bärvåg | Träger |
| synchronous AM (SAM) | AM detected locked to the carrier | synkroninen AM | synkron AM | Synchron-AM |
| AGC | automatic gain control | AGC (automaattinen vahvistuksen säätö) | AGC (automatisk förstärkningsreglering) | AGC (automatische Verstärkungsregelung) |
| hang (AGC) | how long the gain is held after a signal drops | pitoaika | hålltid | Haltezeit |
| decay (AGC) | how fast the gain then rises, dB/s | palautumisnopeus | avklingning | Abklingrate |
| fast / medium / slow (AGC) | AGC speeds | nopea / keski / hidas | snabb / medel / långsam | schnell / mittel / langsam |
| squelch | mutes the audio below a level | kohinasalpa | brusspärr | Rauschsperre |
| noise blanker (NB) | removes impulse noise | pulssihäiriöiden poisto (NB) | störsläckare (NB) | Störaustaster (NB) |
| noise reduction (NR) | reduces steady noise | kohinanvaimennus (NR) | brusreducering (NR) | Rauschminderung (NR) |
| notch | the automatic filter that removes steady carriers | notch-suodin (lovisuodin) (button: Notch) | notchfilter (button: Notch) | Notchfilter |
| volume | audio level | äänenvoimakkuus | volym | Lautstärke |
| mute | | mykistä | tysta | stummschalten |
| stereo / I and Q | IQ mode: I left, Q right | stereo / I ja Q | stereo / I och Q | Stereo / I und Q |
| S-meter | signal strength meter | S-mittari | S-meter | S-Meter |
| overload (ADC) | the ADC's input beyond full scale | ylikuormitus | överstyrning | Übersteuerung |
| attenuator | RF attenuator before the ADC | vaimennin | dämpsats | Abschwächer |
| dither | noise added in the ADC to linearise it | dither | dither | Dither |
| full scale, dBFS | the ADC's largest value; levels relative to it | täysi skaala | full skala | Vollaussteuerung |
| converter | an external frequency converter before the receiver | konvertteri | konverter | Konverter |
| frequency offset | the converter's shift added to shown frequencies | taajuussiirtymä | frekvensförskjutning | Frequenzversatz |

## Display

| English | meaning here | Suomi | Svenska | Deutsch |
|---|---|---|---|---|
| waterfall | the scrolling spectrum display | vesiputous | vattenfall | Wasserfall |
| spectrum | the spectrum trace above it | spektri | spektrum | Spektrum |
| zoom / pan | | zoomaus / panorointi | zooma / panorera | Zoom / verschieben |
| span | the shown frequency range | näyttöleveys | spann | Darstellungsbreite |
| peak hold | each frequency's highest level, kept | huippuarvon pito | topphållning | Spitzenwerthaltung |
| contrast, floor, top | the waterfall's level range (lowest, highest) | kontrasti, alaraja, yläraja | kontrast, golv, tak | Kontrast, Untergrenze, Obergrenze |
| palette, colours | the waterfall's colour scheme | väripaletti, värit | färgpalett, färger | Farbpalette, Farben |
| time marks | UTC times down the waterfall's edge | aikamerkit | tidsmarkeringar | Zeitmarken |
| labels | frequency labels over the spectrum | nimiöt | etiketter | Beschriftungen |
| band ribbon | the band plan strip under the spectrum | kaistanauha | bandremsa | Bandleiste |
| band plan | the bands' allocations | bandijako | bandplan | Bandplan |
| region (IARU) | IARU Region 1, 2, 3 | IARU-alue | IARU-region | IARU-Region |
| data saver | fewer waterfall lines, less data | datansäästö | datasparläge | Datensparmodus |
| codec | how the audio is compressed (Opus, μ-law, PCM) | koodekki | kodek | Codec |
| bin | one channel of the channelizer (the FPGA's filter bank; channels 30 kHz apart, 60 kS/s, about 40 kHz usable) | kanavoijan kanava | kanal | Filterbank-Kanal |
| bin (waterfall) | one point of the waterfall's spectrum (the data saver's "a bin for every two pixels") | taajuuspiste | punkt | Wert |
| channelizer | the FPGA's filter bank that splits the band into bins | kanavoija | kanaliserare | Filterbank |

## Listening together, decoders

| English | meaning here | Suomi | Svenska | Deutsch |
|---|---|---|---|---|
| callsign | an amateur station's call | kutsu | anropssignal | Rufzeichen |
| nickname | a name instead of a call | nimimerkki | smeknamn | Spitzname |
| share (frequency) | others see where you listen | jakaa | dela | teilen |
| listen along | hear another listener's channel | kuunnella mukana | lyssna med | mithören |
| queue | waiting for a free receiver | jono | kö | Warteschlange |
| memories | stored frequencies | muistipaikat | minnen | Speicher |
| spot | a station reported heard (DX cluster, skimmers) | spotti | spot | Spot |
| skimmer, background decoder | the receiver's own always-on decoders | taustadekooderi | bakgrundsavkodare | Skimmer ("background decoders": Hintergrund-Dekoder) |
| slot (FT8, WSPR) | one decoding period, 15 s for FT8 | jakso | period | Zeitschlitz |
| greyline | the day–night terminator on the map | greyline | grålinje | Greyline |
| decoder | | dekooderi | avkodare | Dekoder |
| shift (FSK) | mark–space distance, Hz | siirto | skift | Shift |
| rate, baud rate | symbol rate | nopeus | hastighet, baudhastighet | Baudrate |
| baud | symbol rate | baudi | baud | Baud |
| reverse (FSK) | mark and space swapped | käänteinen | omvänd | invertiert |
| slant (fax, SSTV) | image skew from a clock error | vinous | snedhet | Schräglauf |
| signal identifier | names the mode of a signal | signaalin tunnistus | signalidentifiering | Signalerkennung |
| screen reader | software that reads the page aloud | ruudunlukija | skärmläsare | Screenreader |
| accessibility | | saavutettavuus | tillgänglighet | Barrierefreiheit |

## Propagation

| English | meaning here | Suomi | Svenska | Deutsch |
|---|---|---|---|---|
| propagation | HF propagation | kelit (button: Kelit) | vågutbredning (button: Prognos) | Ausbreitung, KW-Ausbreitung (button: Prognose) |
| MUF | maximum usable frequency | suurin käyttökelpoinen taajuus | högsta användbara frekvens | höchste brauchbare Frequenz |
| forecast | | ennuste | prognos | Vorhersage |
| reliability | % of days a path works for a mode | luotettavuus | tillförlitlighet | Zuverlässigkeit |
| sunspot number (SSN) | | auringonpilkkuluku | solfläckstal | Sonnenfleckenzahl |
| Kp index | planetary geomagnetic index | Kp-indeksi | Kp-index | Kp-Index |
| geomagnetic storm | | geomagneettinen myrsky | geomagnetisk storm | geomagnetischer Sturm |
| path | from a region to the receiver | yhteysväli | förbindelse | Funkstrecke |

## The admin pages

| English | meaning here | Suomi | Svenska | Deutsch |
|---|---|---|---|---|
| admin page | the owner's pages | ylläpito | administration | Verwaltung |
| owner | the receiver's owner | omistaja | ägare | Betreiber |
| bitstream | the FPGA's configuration | bittivirta (FPGA) | bitström | Bitstream |
| slot (A/B) | one of the two system partitions | osio (root slot: juuriosio) | partition (systempartition) | Slot |
| receive part ("the radio") | the unit's receiving hardware and its processing | vastaanotto-osa | mottagardel | Empfangsteil |
| update | an OS update | päivitys | uppdatering | Aktualisierung |
| boot loader | | käynnistyslatain | startladdare | Bootloader |
| safe mode | started without the radio, to recover | vikasietotila | felsäkert läge | abgesicherter Modus |
| backup / restore | the settings file | varmuuskopio / palautus | säkerhetskopia / återställning | Sicherung / Wiederherstellung |
| session | one listener's connection | istunto | session | Sitzung |
| ban | block an address | estää | spärra | sperren |
| API token | a secret a program uses instead of a password | API-tunniste | API-token | API-Token |
| scope (of a token) | what it may do | oikeudet | behörighet | Umfang |
| revoke | make a token stop working | perua | återkalla | widerrufen |
