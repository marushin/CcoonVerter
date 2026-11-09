# CcooNverter user guide

CcooNverter turns folders of RAW photos into JPEG, PNG, TIFF or WebP files. It does not manage photos or do local edits. You pick a folder, check the result in the preview, and press one button.

This guide is written for photographers seeing the app for the first time. More answers are in the [FAQ](FAQ.en.md). 한국어: [GUIDE.ko.md](GUIDE.ko.md)

![Main window](screenshots/gui_windows.png)

## 1. Install

Get the file for your computer from the [Releases](https://github.com/marushin/CcoonVerter/releases/latest) page.

| System | File | How to install |
|---|---|---|
| Windows 10/11 (64-bit) | `CcooNverter_v1.0.0_windows_x64.exe` | Nothing to install. Put the file in any folder and run it. |
| macOS 11 or later (Apple Silicon and Intel) | `CcooNverter_v1.0.0_macos_universal.dmg` | Open the DMG and drag `CcooNverter.app` to Applications. |

### The warning on first launch

Windows and macOS will warn you the first time you open the app, because **it does not yet have a paid code-signing certificate**. Code signing is a way for the operating system's maker to vouch for who built a file. A missing signature does not mean the file is malicious, but the operating system cannot tell who made it, so the warning is expected.

Whether to trust it is your call. The only official download location is the Releases page above, the `SHA256SUMS.txt` file published next to the downloads lets you check that a file has not been changed, and all of the source code is public.

**Windows (SmartScreen)**

If a blue "Windows protected your PC" window appears:

1. Click **More info**.
2. Click **Run anyway**, which appears at the bottom.

Windows will not ask again for that file.

**macOS (Gatekeeper)**

If you see "unidentified developer" or "Apple could not verify", click **Cancel** first and then do one of these:

- macOS 15 (Sequoia) or later: go to **System Settings → Privacy & Security**, scroll down to the Security section and click **Open Anyway**, then enter your password.
- macOS 14 or earlier: in Finder, **Control-click (or right-click) `CcooNverter.app` → Open → Open**.
- If those do not work, or the message says the app "is damaged and can't be opened", run this line in Terminal. It removes the quarantine mark that macOS adds to downloaded files.

  ```bash
  xattr -dr com.apple.quarantine /Applications/CcooNverter.app
  ```

This guide was written on Windows; these macOS steps have not been carried out on a Mac by the author of this document.

### Language

The app starts in Korean or English, following your system language. You can change it in the **App** section at the bottom left.

## 2. Your first conversion

**Step 1. Choose a folder.** Drop a folder of RAW files on the window, or use **Browse** at the top. Sub-folders are included. To convert just a few photos, drop those RAW files on the window instead.

![Empty window](screenshots/empty_en.png)

**Step 2. Look at the preview.** The first photo is developed in the middle of the window. Use `<` and `>` to look at others.

**Step 3. Press the button.** Press **Let's Get Cooking!** at the bottom right. Progress and the remaining time are shown, and a result card appears when it is done.

If you did not choose a save folder, results go to a **`Converted_JPG` folder inside your RAW folder**, keeping any sub-folder structure. Your RAW files are never changed.

## 3. RAW develop or Camera JPEG

Two buttons at the top left choose the mode.

| | **RAW develop** | **Camera JPEG** |
|---|---|---|
| What it does | Develops the RAW data itself. | Takes the JPEG the camera saved inside the RAW. |
| Adjustments (exposure, styles...) | Applied. | Not applied, and hidden from the window. |
| Speed | Slower. | The fastest. |
| Output | JPEG, PNG, 16-bit TIFF or WebP | Always JPEG |
| Cameras the app cannot develop | Reported as failed. | Usually still works. |

In **Camera JPEG** mode with no long edge set, the camera's image data is stored as it is, without compressing it again, so there is no quality loss. If you set a long edge it has to be compressed again to change its size. If a file has no camera JPEG inside, the RAW is developed instead and the result card tells you.

As a rule of thumb: Camera JPEG for quick sharing with the camera's own colours, RAW develop when you want to adjust colour and brightness or edit further.

## 4. Adjustments and styles

These appear in RAW develop mode. The preview updates as you change a value.

- **Exposure (EV)**: overall brightness. +1 doubles the light. The app starts from the brightness of the camera's JPEG, so 0 is usually the right place to begin.
- **White balance**: *Camera Default* uses the setting at capture, *Auto* estimates it from the picture.
- **Tone Curve**: a gentle contrast curve similar to a camera JPEG. Turn it off for a flat result.
- **Style**: choose among 12 thumbnails. They show your current photo in each style.
- **Advanced**: click to open RGB balance and black and white points (levels). A brightness value from an older preset also appears here.
- **Reset**: returns only the RAW adjustments to their defaults. Size, quality, EXIF and the other output settings stay.

**Double-click** a slider to return just that value to its default.

![Advanced](screenshots/advanced_en.png)

## 5. Preview tools

The preview goes through the same processing as the saved file. What you see is what gets written.

- **Fit / 100%**: Fit shows the whole photo, 100% shows real pixels. Double-clicking the photo switches too. At 100%, drag to move around. The first switch to 100% on a large RAW can take a few seconds.
- **Compare**: the camera's JPEG on the left, your development on the right. Drag the divider to move it. Available only when the file contains a camera JPEG.
- **Histogram**: brightness distribution at the top right. The button shows or hides it.

![Compare](screenshots/split_en.png)

In a narrow window the preview is opened with the **Show preview** button. The preview pauses while a conversion runs.

## 6. Output options

Set these in the **Output** section on the left.

- **Long edge (px)**: pixels on the long side of the output. Leave it empty for the **original size**. A value larger than the original does not enlarge the photo.
- **Quality**: applies to JPEG and WebP. 90 is usually plenty.
- **Keep EXIF shooting data**: copies camera, lens, exposure, date, GPS and rating, as far as the file has them, into the output. **It is on by default.** Location data is copied too, so turn this off before posting photos online if you want it removed.
- **Keep source file date**: gives the output the same modified date as its RAW file.
- **If a file exists**: what to do when an output with the same name is already there: *Add suffix* (saves a new file such as `name_arw.jpg`), *Skip* or *Overwrite*.

The **Format** button opens the detailed options.

![Output options](screenshots/output_en.png)

| Format | Notes |
|---|---|
| JPEG | 8-bit. Small files that open everywhere. |
| PNG | 8-bit lossless. Large files. |
| TIFF 16-bit | 16 bits per channel, uncompressed. For further editing; very large. |
| WebP | 8-bit lossy. Smaller than JPEG. Up to 16383 pixels per side. |

Leave **Color space** on *sRGB* unless you have a reason not to. *AdobeRGB* is meant for print workflows but has a limit, explained in the [FAQ](FAQ.en.md#does-saving-in-adobergb-give-me-wider-colours).

**File name** can contain the items below. For example, `{date}_{name}` gives `2026-11-09_IMG_1234.jpg`. The dialog shows a live example.

| Item | Meaning |
|---|---|
| `{name}` | Original file name (the default) |
| `{date}` | Capture date `YYYY-MM-DD`, or the file date if there is none |
| `{time}` | Capture time `HHMMSS` |
| `{seq}` | Sequence number from `0001` |
| `{camera}` | Camera model |
| `{ext}` | Original file extension |

## 7. Presets

Save the settings you use often under a name. In **Presets** at the bottom left, press **Save** and type a name. Choosing it from the list later applies everything saved, except "If a file exists", which a preset never changes.

Presets from the old Python version (v0.8.x) can be read too.

## 8. Reading the result card

When a conversion ends, a card takes the place of the preview.

![Result card](screenshots/result_en.png)

- **Converted**: files saved successfully.
- **Skipped**: files not written because of "If a file exists: Skip".
- **Failed**: files that could not be converted. Their names and the reasons are listed below.

What the reasons mean:

| On the card | Meaning | What you can do |
|---|---|---|
| RAW development does not support this camera yet | The decoder does not know this camera. | Camera JPEG mode can still take the JPEG out. |
| This RAW compression cannot be developed yet | A compression such as Nikon High Efficiency. | Same as above. |
| The file is damaged or is not a RAW file this app can read | The file is broken or not a RAW file. | Check whether another program can open it. |
| Could not read or write the file | The disk is full or permissions are missing. | Check the save location. |

**Open folder** opens the output folder. **Close** returns to the preview. If some files failed, the card also shows where the log with more detail is.

## 9. Where settings and logs live

Settings are saved when you close the app, and it starts again with your last folder and settings.

| System | Settings and presets | Log |
|---|---|---|
| Windows | `%APPDATA%\ccoon\CcooNverter\config\` | `%LOCALAPPDATA%\ccoon\CcooNverter\data\logs\ccoonverter.log` |
| macOS | `~/Library/Application Support/ccoon.CcooNverter/` | `~/Library/Application Support/ccoon.CcooNverter/logs/ccoonverter.log` |

`settings.json` holds the settings, `presets.json` the presets. If you report a problem, please attach the log. To remove the app, delete the executable and those folders.

The app uses the internet for one thing only: checking GitHub for a new version, at most once a day. Photos and usage data are never sent anywhere. The checkbox in the **App** section at the bottom left turns it off.
