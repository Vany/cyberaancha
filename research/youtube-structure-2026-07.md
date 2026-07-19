# YouTube page structure — verified in-browser 2026-07-19

Live-probed on youtube.com (logged-out automated Chrome) against @vanyserezhkin.
Corrects the endpoint research in `p0-findings.md` with the *current* JSON shapes.

## Channel listing moved to lockupViewModel

`videoRenderer` / `gridVideoRenderer` are **gone** from channel tabs. Each item
is now a `lockupViewModel` (30 per page, wrapped in `richItemRenderer` inside
`richGridRenderer`). Paths the collector relies on:

- `lm.contentId` → 11-char video id
- `lm.contentType === "LOCKUP_CONTENT_TYPE_VIDEO"` (filter)
- `lm.metadata.lockupMetadataViewModel.title.content` → title
- `…metadata.contentMetadataViewModel.metadataRows[].metadataParts[].text.content`
  → free-text parts; the one matching `/ago|назад/` is the relative publish date
- `lm.contentImage.…overlays[].thumbnailBottomOverlayViewModel.badges[]
  .thumbnailBadgeViewModel.text` → duration like `3:54:32`

Verified: 30 items/page, `continuationCommand.token` drives `/youtubei/v1/browse`,
second page returned another 30 (60 total), durations + titles parse, relative
dates → 30/30 approx timestamps.

## Player endpoint unchanged and working

`POST /youtubei/v1/player {context, videoId}`:
- `videoDetails.title` / `.shortDescription` / `.lengthSeconds` / `.viewCount` / `.channelId`
- `microformat.playerMicroformatRenderer.publishDate` → exact ISO date (e.g. `2026-05-18T06:46:00-07:00`)
- `captions.playerCaptionsTracklistRenderer.captionTracks[]` → `.baseUrl` (+`&fmt=json3`), `.languageCode`, `.kind` (`asr`|absent)

## Notes / gotchas

- **@vanyserezhkin recent uploads are caption-less livestream VODs** — 12 scanned,
  0 caption tracks. The `{none:true}` path is common here; the professor's channel
  (auto-RU captions) will exercise the json3 parse. json3 shape (`events[].segs[].utf8`,
  `tStartMs`, `dDurationMs`) unchanged from research, not yet run end-to-end on real captions.
- **Local-dev can't drive the browser→server POST**: recent Chrome gates
  HTTPS-page → `127.0.0.1` calls behind a Local Network Access permission that
  automation never grants. Irrelevant in prod (youtube.com → aancha.serezhkin.com,
  both public HTTPS). Server round-trip proven via unit tests + curl instead.
- Structure drifts; the collector reads live `ytcfg` and uses forgiving deep-scans,
  but expect to re-verify these paths periodically.
