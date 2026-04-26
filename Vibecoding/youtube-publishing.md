# Publishing AidBooks audiobooks to YouTube

This document covers two things:

1. **Setting up a Google account, project, and credentials** so that
   AidBooks can upload videos to a YouTube channel on a user's behalf.
2. **Designing the publishing flow inside AidBooks** — what to build, where
   it fits in the existing architecture, and the gotchas to plan for.

---

## Part 1 — Get your YouTube credentials

> **Heads-up about "API key":** YouTube *uploads* cannot be authorized with
> a static API key. They require **OAuth 2.0** on behalf of the YouTube
> channel owner. An API key is only useful for read-only public data (e.g.
> looking up a video's metadata). Everything below sets up the OAuth client
> AidBooks needs.

### 1.1 Create or sign in to a Google account

1. Open <https://accounts.google.com/signup>.
2. Use a real, recoverable mailbox (this account owns the project, the
   billing record, and the OAuth verification record). A throwaway address
   will lock you out of the channel later.
3. Enable 2-step verification before going further (Account → Security).

### 1.2 Create the YouTube channel that will host the audiobooks

1. Sign in at <https://www.youtube.com/>.
2. Click your avatar → **Settings → Create a new channel** (or "Add or
   manage your channel(s)" if you already have one).
3. Pick a **Brand account** rather than a personal channel — brand
   accounts can have multiple owners/managers, which makes handoff and
   revocation possible without rotating your password.
4. Note the channel ID (Settings → Advanced settings). You'll want it for
   sanity-checking later.

### 1.3 Create a Google Cloud project

1. Open <https://console.cloud.google.com/projectcreate>.
2. Project name: e.g. `aidbooks-publish`. Organization: leave blank for
   personal use.
3. Wait ~30 seconds for provisioning.
4. (If your account was just created): you may be asked to enable
   billing. The YouTube Data API itself is free up to its quota
   (10 000 units/day default; one upload costs 1600 units → ~6 uploads/day),
   so a billing account isn't strictly required, but Google sometimes
   pushes you through the dialog. Add a card or skip.

### 1.4 Enable the YouTube Data API v3

1. In the console: **APIs & Services → Library**.
2. Search **"YouTube Data API v3"** → click → **Enable**.

### 1.5 Configure the OAuth consent screen

This is the dialog users will see when they click "Connect YouTube" in
AidBooks.

1. **APIs & Services → OAuth consent screen**.
2. **User type**: pick **External**. (Internal is only available if your
   account is in a Google Workspace org and you only want users in that
   org to authorize.)
3. **App information**:
   - App name: `AidBooks`
   - User support email: your address
   - App logo: optional, but improves trust
4. **App domain**: leave blank for now if AidBooks isn't on a public
   domain yet — you can fill it in before submitting for verification.
5. **Authorized domains**: empty for development; add `your-domain.tld`
   once you have one.
6. **Developer contact**: your email.
7. **Scopes**: click **Add or remove scopes** and add:
   - `https://www.googleapis.com/auth/youtube.upload` — the only one
     strictly needed for uploads.
   - `https://www.googleapis.com/auth/youtube.readonly` — useful if you
     want to read back channel info / list uploads.
   These are flagged **sensitive** by Google.
8. **Test users**: while the app is unverified you can only authorize
   accounts listed here. Add your own Google address. Up to 100 testers
   is allowed.
9. Click **Save and continue**, then **Back to dashboard**.

### 1.6 Create the OAuth 2.0 Client ID

1. **APIs & Services → Credentials → Create Credentials → OAuth client
   ID**.
2. **Application type**: **Web application**.
3. **Name**: `AidBooks backend` (just a label).
4. **Authorized redirect URIs**: this is the URL on your backend that
   Google will redirect to after the user consents. For local dev, add:
   - `http://localhost:8787/integrations/youtube/oauth/callback`
   For production, also add your real URL there. Wildcard subdomains are
   not allowed; list each environment explicitly.
5. **Create**. The dialog shows the **Client ID** and **Client secret**
   — copy them now (the secret is shown only once; you can rotate it
   later).

You now have everything AidBooks needs:
```
LISTENAI_YOUTUBE_CLIENT_ID=<the client id>
LISTENAI_YOUTUBE_CLIENT_SECRET=<the client secret>
LISTENAI_YOUTUBE_REDIRECT_URI=http://localhost:8787/integrations/youtube/oauth/callback
```

### 1.7 Verification (when going beyond test users)

The `youtube.upload` scope is **sensitive**, so production usage above
the 100 test-user cap requires Google to verify your app. Plan for:

- A privacy policy URL on a real domain.
- A terms of service URL.
- A short demo video showing how AidBooks uses the scope.
- Justification text explaining *why* you need upload access.
- Turnaround: usually 4–6 weeks; sometimes longer if Google asks for
  changes. Don't put this on the critical path of a launch.

You can ship to friends and yourself in the meantime by adding their
Google addresses to the **Test users** list.

### 1.8 Quotas to plan around

- Default per-project quota: **10 000 units / day**.
- An upload (`videos.insert`) costs **1600 units**. → ~6 uploads/day per
  project before you hit the wall.
- A read (`videos.list`) costs **1 unit**.
- You can request a quota increase via the Cloud Console quotas page;
  Google grants it case-by-case. Allow 2–4 weeks.

---

## Part 2 — Integrating publishing into AidBooks

This section is a design sketch that maps the publishing flow onto
AidBooks' existing architecture (Axum + SurrealDB + a job worker pool +
the per-audiobook storage tree). Nothing here is wired up yet — implement
in this order.

### 2.1 What "publish" actually produces

YouTube only ingests **video** containers, not raw audio. The output of a
publish job is a single MP4 per audiobook with:

- **Audio track**: every chapter WAV concatenated, in order (with a short
  silence between).
- **Video track**: the audiobook's cover image at 1920×1080 (or 1280×720
  for low bitrate), held for the full duration. Optionally, a chapter
  title overlay that changes at chapter boundaries.

A 1-hour audiobook with a static cover encodes to roughly 30–80 MB at
sensible bitrates — well below YouTube's 256 GB / 12 h cap.

### 2.2 Configuration

Add to `core/src/config.rs`:

```rust
pub youtube_client_id: String,
pub youtube_client_secret: String,
pub youtube_redirect_uri: String,
/// Path to the `ffmpeg` binary. Empty string disables publishing.
pub ffmpeg_bin: String,
```

Defaults all empty so existing deployments keep working unchanged. Mirror
in `.env.example` and document the env var names.

### 2.3 Database

One new migration adds the per-user OAuth token table and an
audiobook-level publication record:

```surql
DEFINE TABLE IF NOT EXISTS youtube_account SCHEMAFULL;
DEFINE FIELD owner ON youtube_account TYPE record<user>;
DEFINE FIELD channel_id ON youtube_account TYPE string;
DEFINE FIELD channel_title ON youtube_account TYPE string;
DEFINE FIELD refresh_token_enc ON youtube_account TYPE string;  -- AEAD-encrypted
DEFINE FIELD scopes ON youtube_account TYPE array<string>;
DEFINE FIELD connected_at ON youtube_account TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE INDEX yt_owner ON youtube_account FIELDS owner UNIQUE;

DEFINE TABLE IF NOT EXISTS youtube_publication SCHEMAFULL;
DEFINE FIELD audiobook ON youtube_publication TYPE record<audiobook>;
DEFINE FIELD language ON youtube_publication TYPE string;        -- one row per language version
DEFINE FIELD video_id ON youtube_publication TYPE option<string>;
DEFINE FIELD privacy_status ON youtube_publication TYPE string
    ASSERT $value IN ["private", "unlisted", "public"];
DEFINE FIELD video_url ON youtube_publication TYPE option<string>;
DEFINE FIELD published_at ON youtube_publication TYPE option<datetime>;
DEFINE FIELD last_error ON youtube_publication TYPE option<string>;
DEFINE INDEX yt_pub_unique ON youtube_publication
    FIELDS audiobook, language UNIQUE;
```

Encrypt the refresh token at rest with the same secret you already use
for refresh tokens (`Config.password_pepper` or a dedicated AEAD key).
Never log it.

### 2.4 OAuth flow (HTTP handlers)

Two new endpoints under `backend/api/src/handlers/integrations.rs`:

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/integrations/youtube/oauth/start` | Builds the Google consent URL with `state=<random>` (stored in a short-TTL `oauth_state` table to prevent CSRF) and returns a redirect. |
| `GET` | `/integrations/youtube/oauth/callback` | Exchanges the `code` for tokens, fetches the channel info, persists the encrypted `refresh_token`, redirects back to the AidBooks UI. |

Use the standard `code` grant flow with `access_type=offline` and
`prompt=consent` so Google always issues a refresh token.

### 2.5 Adding a new JobKind

Mirror the `Translate` pattern (the timeout fix you just shipped). One
new variant:

```rust
JobKind::PublishYoutube
```

Migration: widen the `job.kind` ASSERT and add `(JobKind::PublishYoutube, 1)`
to the worker pool defaults. One worker is plenty: each upload is
dominated by network upload time; running them in parallel mostly just
fights for the same per-project quota.

### 2.6 The publish handler

`backend/api/src/jobs/publishers/youtube.rs`:

1. Resolve the audiobook + the chapters in the requested language. Bail
   if any chapter isn't `audio_ready`.
2. Refresh the user's OAuth access token (POST to
   `https://oauth2.googleapis.com/token` with the stored refresh token).
3. **Assemble the MP4** under
   `<storage>/<audiobook>/<lang>/youtube.mp4`:
   - Build a concat list of the chapter WAVs.
   - Run `ffmpeg`:
     ```
     ffmpeg -loop 1 -framerate 1 -i cover.png \
            -f concat -safe 0 -i chapters.txt \
            -c:v libx264 -tune stillimage -pix_fmt yuv420p \
            -c:a aac -b:a 192k -shortest \
            -movflags +faststart \
            youtube.mp4
     ```
     Static-image videos compress brilliantly because every frame is
     identical; the `-tune stillimage` preset exploits that.
   - Use `tokio::process::Command` and stream stderr to
     `ctx.progress(&job, "encoding", pct)` by parsing `time=`. Encoding
     a 1-hour audiobook on a modest box takes ~30–60 s.
4. **Upload via the resumable protocol.** Two HTTP calls to YouTube:
   - `POST https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status`
     with the JSON metadata. The response carries an upload URL in the
     `Location` header.
   - `PUT <upload URL>` with the MP4 bytes, in 8 MiB chunks. After each
     chunk, emit `ctx.progress(&job, "uploading", bytes_done / total)`.
     Honor the `Range:` header on a 308 reply to know what to resume
     from.
5. On 200, store `video_id`, `video_url`
   (`https://youtu.be/{video_id}`), and `published_at` on the
   `youtube_publication` row. Emit a terminal `completed` event.

Quota tip: do the **encode** step before requesting the upload URL. If
encoding fails you haven't burned 1600 units.

### 2.7 Metadata mapping

| YouTube field | Source |
|---------------|--------|
| `snippet.title` | `audiobook.title` (translated where applicable) |
| `snippet.description` | Topic + genre + chapter list with timestamps generated from `chapter.duration_ms` running totals. Chapter timestamps in the description make YouTube auto-create chapter markers on the player scrubber. |
| `snippet.tags` | `[genre, language, "audiobook", "AidBooks"]` |
| `snippet.categoryId` | `"22"` (People & Blogs) — safer default than 27 (Education) for AI-narrated content. |
| `snippet.defaultLanguage` | The audiobook's `language` |
| `status.privacyStatus` | User-chosen: `private` / `unlisted` / `public` |
| `status.madeForKids` | Always `false` unless the user explicitly opts in. |

### 2.8 HTTP surface for the UI

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/integrations/youtube/account` | Returns `{connected, channel_title}` so the UI can show the connect/disconnect state. |
| `DELETE` | `/integrations/youtube/account` | Revokes the refresh token (POST to Google's revoke endpoint), deletes the row. |
| `POST` | `/audiobook/:id/publish/youtube` | Body `{language, privacy_status}`. Validates ownership, that the YouTube account is connected, and that the language version is fully narrated; enqueues `JobKind::PublishYoutube` and returns 202 + `{job_id, publication_id}`. |
| `GET` | `/audiobook/:id/publications` | List of `youtube_publication` rows for the audiobook (one per language), with status. |

### 2.9 Frontend touchpoints

- **Settings → Integrations**: a "Connect YouTube" card. Clicking calls
  `/integrations/youtube/oauth/start`, which redirects to Google. After
  consent the user lands back on Settings with a
  `?connected=youtube` query so the page can show a confirmation toast.
- **Audiobook detail page**: in the actions row (next to *Open player*),
  a **Publish to YouTube** button per language tab. Disabled until that
  language is `audio_ready`. Opens a small dialog: privacy radio
  (private / unlisted / public), optional description override, then
  POSTs to `/audiobook/:id/publish/youtube`.
- **Detail page footer**: a "Published" panel listing each
  `youtube_publication` row with link to the video and re-publish action
  (uploads a new video — YouTube doesn't allow replacing the bytes of an
  existing video, only metadata).

### 2.10 Operational considerations

- **Token refresh**: cache the access token in process memory for its
  TTL (typically 60 minutes); fall back to refresh on 401.
- **Refresh-token revocation**: Google may invalidate it if the user
  changes their password or revokes from <https://myaccount.google.com/permissions>.
  Handle 400 `invalid_grant` by deleting the row and asking the user to
  reconnect.
- **Cover image only**: AidBooks doesn't ship FFmpeg in the backend
  Docker image yet. Either install it in the runtime image (`apk add
  ffmpeg`) or use a sidecar service. The `Config.ffmpeg_bin` knob lets
  you point at a custom path.
- **Cost**: encoding is CPU-bound; uploads are network-bound. With one
  worker, both are bounded. If you ever scale workers, throttle them
  against your daily 10 000-unit quota — a busy queue can exhaust it in
  under a minute.
- **Content-ID**: AI-generated audio can occasionally trigger false
  positives if your TTS voice resembles a copyrighted reading. Surface
  the `processingDetails.processingFailureReason` field after upload so
  the UI can warn the user when YouTube flags the video.
- **Deletion**: deleting an audiobook in AidBooks should *not*
  automatically delete the YouTube video — videos are public artifacts
  the user expects to outlive their library row. Just clean up the
  `youtube_publication` row and let the user delete on YouTube manually.

### 2.11 Build order

Suggested commit order, each independently shippable:

1. Config + migration + `youtube_account` table + OAuth start/callback
   handlers. Land "Connect YouTube" on Settings; nothing publishes yet.
2. `JobKind::PublishYoutube` + handler skeleton that just builds the
   metadata and logs (skip encode + upload). Verifies the job pipeline.
3. FFmpeg encode step. Validate the output by playing
   `youtube.mp4` locally.
4. Resumable upload step. Test against a private video first, then
   unlisted, then public.
5. Detail-page UI: button, dialog, publication list.
6. Polish: chapter-marker timestamps, retry on `invalid_grant`,
   revocation handling.
