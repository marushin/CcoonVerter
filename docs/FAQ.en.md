# CcooNverter FAQ

For how to use the app, see the [user guide](GUIDE.en.md). 한국어: [FAQ.ko.md](FAQ.ko.md)

## Why is there a warning on first launch?

Because the app has no paid code signature yet. Windows SmartScreen and macOS Gatekeeper warn about unsigned programs the first time they run. Signing needs a paid certificate and account that are not in place yet.

A warning does not mean the file is dangerous, but it does not prove it is safe either. Make sure you downloaded it from the official place (GitHub Releases) and, if you like, compare it with the published `SHA256SUMS.txt`. How to allow it is in the [guide](GUIDE.en.md#the-warning-on-first-launch).

## Does it ever change my RAW files?

No. RAW files are only ever opened for reading. The only files the app writes are the converted photos, settings, presets, a log and the update check record. Converted files always end in `.jpg`, `.png`, `.tif` or `.webp`, so no file name template or "Overwrite" setting can replace an `.ARW`, `.CR3`, `.NEF` or other RAW file.

"Overwrite" does replace **a previous conversion result**, so be careful if you edited files in the output folder by hand.

## My camera is reported as unsupported

RAW decoding uses the open-source library [rawler](https://github.com/dnglab/dnglab), version 0.8.0. These files cannot be developed at the moment:

- **Nikon High Efficiency and High Efficiency\* NEF** (taken with that compression on a Z6III, Z f and similar). Lossless compressed NEF develops normally.
- **Sony A7 V (ILCE-7M5) ARW**.

For those files, **Camera JPEG mode still works**: it takes the JPEG the camera saved inside the RAW, and the result card says so. The app does not switch modes by itself; the choice is yours. Support can be added once rawler can read these formats.

The cameras used for verification are listed in the [README](../README.md#supported-formats).

## Why are the colours different from the camera's?

A camera JPEG is made with the manufacturer's private colour processing (picture styles, film simulations and so on). This app does not imitate that; it develops using the colour data in the RAW. So the same photo looks a little different. Neither is wrong, they are different developments.

**Brightness** starts from the camera JPEG in the file, so it is usually close. **Colour** is not matched to the camera. If you need exactly the camera's colours, use **Camera JPEG mode**. The **Compare** button shows both side by side. Fujifilm X-Trans files currently use a simple colour reconstruction, so fine detail can look a little soft.

## Does saving in AdobeRGB give me wider colours?

**Not by much.** The file is correct AdobeRGB with an ICC profile and shows accurately in colour-managed software. But the decoder clips colours to the sRGB range before the conversion to AdobeRGB happens, so colours outside sRGB are already gone by then. Only saturation added later by styles or RGB balance can go beyond sRGB.

If you need a truly wide gamut for printing, another developer is the better choice. For most uses sRGB is plenty. The preview always shows the sRGB rendering.

## I kept EXIF but the MakerNote is missing

The MakerNote is private data in a format of each manufacturer's own. Its contents point to positions inside the file, so it tends to break when moved to another file.

- When the camera JPEG's EXIF can be used as it is, the MakerNote comes along.
- When GPS or lens data missing from the camera JPEG has to be **merged in from the RAW, the MakerNote is not kept**.
- TIFF output never has it.

Common data such as capture date, camera, lens, exposure, GPS and rating is copied as far as the file has it. Ratings can only be read when stored as XMP.

## Can a damaged file close the app?

**Yes, it can.** Most broken RAW files are simply reported as failed and the batch carries on. But occasionally the library that reads the RAW (rawler) trusts a bad value and asks for tens of gigabytes of memory, or keeps growing its memory without end. The operating system then ends the whole app, and nothing inside the app can stop that. The most common cases are checked and refused before the library sees the file, but not all of them. Test runs that feed the app large numbers of randomly corrupted files can still make this happen.

This is unlikely with photos from a camera. If it does happen, **files already saved and your RAW files are untouched**. To continue, look for "Converting ..." near the end of the log file to find the suspect file, move it elsewhere, and run again with "If a file exists: Skip".

The real fix is to decode RAW files in a separate process. It is planned as a future improvement.

## The conversion seems stuck or slow

Developing one RAW takes a lot of memory (about 0.7 GB for 24 MP), so the app decides how many to process at once from the memory available. The remaining time can take a few files to settle. **Cancel** stops after the current files have been dealt with.

## Does it send my photos anywhere?

No. The internet is only used to check GitHub for a new version, at most once a day. A checkbox at the bottom left turns that off.

## What is the licence?

CcooNverter is under the MIT licence. Licences of the libraries it uses are in [THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md).
