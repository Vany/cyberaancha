// aancha collector — runs in youtube.com page context (SPEC §11).
// Pure fetch, zero DOM script injection: works under YouTube's CSP/Trusted Types.
// Reads live ytcfg (never hardcodes client versions), paces every YouTube
// request, posts JSON to the aancha server. Config comes from window.AANCHA_CFG
// = { server, token, pace_ms } set by the snippet/bookmarklet before this file.
(() => {
  "use strict";
  const cfg = window.AANCHA_CFG;
  if (!cfg || !cfg.server || !cfg.token) {
    alert("aancha: window.AANCHA_CFG {server, token} required");
    return;
  }
  if (!location.hostname.endsWith("youtube.com")) {
    alert("aancha: run this on a youtube.com page");
    return;
  }
  if (window.AANCHA_RUNNING) { console.warn("aancha: already running"); return; }
  window.AANCHA_RUNNING = true;
  window.AANCHA_STOP = false;

  // ---- tiny UI --------------------------------------------------------------
  const box = document.createElement("div");
  box.style.cssText =
    "position:fixed;bottom:12px;right:12px;z-index:99999;background:#111;color:#8f8;" +
    "font:12px/1.5 monospace;padding:10px 14px;border-radius:8px;max-width:340px;" +
    "box-shadow:0 2px 12px rgba(0,0,0,.5);white-space:pre-wrap";
  box.textContent = "aancha: starting…";
  const stopBtn = document.createElement("button");
  stopBtn.textContent = "stop";
  stopBtn.style.cssText = "margin-left:10px;background:#611;color:#fff;border:0;border-radius:4px;padding:1px 8px;cursor:pointer";
  stopBtn.onclick = () => { window.AANCHA_STOP = true; say("stopping after current task…"); };
  document.body.append(box);
  box.append(stopBtn);
  const say = (msg) => { box.childNodes[0].textContent = "aancha: " + msg; };

  // Verbose console logging (on by default; set AANCHA_CFG.debug=false to quiet).
  // Filter the console by "aancha" to follow along.
  const DEBUG = cfg.debug !== false;
  const dbg = (...a) => { if (DEBUG) console.log("%caancha", "color:#3ba;font-weight:bold", ...a); };
  const dwarn = (...a) => console.warn("aancha:", ...a);
  dbg("collector loaded", { server: cfg.server, pace_ms: cfg.pace_ms || 1500, loggedIn: /SAPISID=/.test(document.cookie) });

  // ---- helpers --------------------------------------------------------------
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const pace = async () => sleep((cfg.pace_ms || 1500) + Math.random() * 700);

  const api = async (path, opts = {}) => {
    const res = await fetch(cfg.server + path, {
      ...opts,
      headers: {
        authorization: "Bearer " + cfg.token,
        "content-type": "application/json",
        ...(opts.headers || {}),
      },
    });
    if (!res.ok) throw new Error(`server ${path} -> ${res.status}: ${await res.text()}`);
    return res.status === 204 ? null : res.json();
  };

  const ytcfgGet = (key) => window.ytcfg && window.ytcfg.get ? window.ytcfg.get(key) : null;

  // Everything we harvest is PUBLIC (videos, captions, comments, chat replays),
  // so we deliberately do NOT use the logged-in session: `credentials: "omit"`
  // makes every request behave exactly like a logged-out browser (the path we
  // verified works). Sending a SAPISIDHASH auth header on a logged-in session
  // was breaking these public calls — losing everything. Owner-only/private data
  // (P7) will opt back into the session behind a flag; public harvest never needs it.
  const innertube = async (endpoint, body, creds = "omit") => {
    await pace();
    const ctx = ytcfgGet("INNERTUBE_CONTEXT");
    if (!ctx) throw new Error("no INNERTUBE_CONTEXT on this page — open any watch/channel page");
    const res = await fetch(`${location.origin}/youtubei/v1/${endpoint}?prettyPrint=false`, {
      method: "POST",
      credentials: creds,
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ context: ctx, ...body }),
    });
    if (!res.ok) { dwarn(`innertube ${endpoint} (${creds}) -> ${res.status}`); throw new Error(`innertube ${endpoint} -> ${res.status}`); }
    return res.json();
  };

  const fetchPage = async (path) => {
    await pace();
    const res = await fetch(location.origin + path, { credentials: "omit" });
    if (!res.ok) throw new Error(`page ${path} -> ${res.status}`);
    return res.text();
  };

  const extractJson = (html, marker) => {
    const at = html.indexOf(marker);
    if (at < 0) return null;
    const start = html.indexOf("{", at);
    // Balance braces respecting strings — the blob is huge but well-formed.
    let depth = 0, inStr = false, esc = false;
    for (let i = start; i < html.length; i++) {
      const ch = html[i];
      if (esc) { esc = false; continue; }
      if (ch === "\\") { esc = true; continue; }
      if (ch === '"') inStr = !inStr;
      if (inStr) continue;
      if (ch === "{") depth++;
      else if (ch === "}" && --depth === 0) return JSON.parse(html.slice(start, i + 1));
    }
    return null;
  };

  // "3 недели назад" / "3 weeks ago" → approximate ISO timestamp (batching
  // heuristic only; exact dates come from harvest_meta).
  const UNITS = {
    "секунд": 1, "second": 1, "минут": 60, "minute": 60, "час": 3600, "hour": 3600,
    "дн": 86400, "ден": 86400, "day": 86400, "недел": 604800, "week": 604800,
    "месяц": 2592000, "month": 2592000, "год": 31536000, "лет": 31536000, "year": 31536000,
  };
  const approxDate = (text) => {
    if (!text) return null;
    const m = text.match(/(\d+)\s*(\S+)/);
    if (!m) return null;
    const unit = Object.keys(UNITS).find((u) => m[2].toLowerCase().startsWith(u));
    if (!unit) return null;
    return new Date(Date.now() - m[1] * UNITS[unit] * 1000).toISOString();
  };

  const deepFind = (node, key, out = []) => {
    if (!node || typeof node !== "object") return out;
    if (Array.isArray(node)) { node.forEach((n) => deepFind(n, key, out)); return out; }
    if (node[key] !== undefined) out.push(node[key]);
    Object.values(node).forEach((v) => deepFind(v, key, out));
    return out;
  };

  // ---- task handlers --------------------------------------------------------
  // YouTube's channel tabs render each item as a lockupViewModel (verified
  // 2026-07; replaced videoRenderer/gridVideoRenderer). Exact truth (dates,
  // full title) comes later from harvest_meta; here we only need id + window.
  const parseLockups = (root, kind, seen, out) => {
    for (const lm of deepFind(root, "lockupViewModel")) {
      const id = lm.contentId;
      if (!id || lm.contentType !== "LOCKUP_CONTENT_TYPE_VIDEO" || seen.has(id)) continue;
      seen.add(id);
      const meta = lm.metadata?.lockupMetadataViewModel;
      const title = meta?.title?.content || "";
      let publishedText = null;
      for (const part of deepFind(meta, "metadataParts").flat()) {
        const t = part?.text?.content;
        if (t && /ago|назад/i.test(t)) { publishedText = t; break; }
      }
      let dur = null;
      for (const badge of deepFind(lm.contentImage, "thumbnailBadgeViewModel")) {
        if (/^(\d+:)?\d?\d:\d\d$/.test(badge.text || "")) {
          dur = badge.text.split(":").reduce((a, p) => a * 60 + (+p || 0), 0);
          break;
        }
      }
      out.push({
        yt_id: id, kind,
        title: title.slice(0, 500),
        approx_published: approxDate(publishedText),
        duration_s: dur,
      });
    }
  };

  const discover = async (input) => {
    const videos = [];
    const seen = new Set();
    let channelId = null;
    for (const [tab, kind] of [["videos", "video"], ["streams", "stream"]]) {
      say(`discover: /${tab} …`);
      let html;
      try { html = await fetchPage(`/${input.handle}/${tab}`); }
      catch { continue; } // channels without a streams tab
      const data = extractJson(html, "ytInitialData");
      if (!data) continue;
      channelId = channelId ||
        data?.metadata?.channelMetadataRenderer?.externalId ||
        data?.header?.pageHeaderRenderer?.content?.pageHeaderViewModel?.channelId || null;
      parseLockups(data, kind, seen, videos);
      let tokens = deepFind(data, "continuationCommand").map((c) => c.token).filter(Boolean);
      // Walk continuations until the tab is exhausted (whole listing is cheap).
      // A failed continuation must NOT discard the videos already found — keep
      // what we have and move on (page 1 alone is a usable result).
      while (tokens.length && !window.AANCHA_STOP) {
        say(`discover: /${tab} … ${seen.size} items`);
        let page;
        try { page = await innertube("browse", { continuation: tokens[0] }); }
        catch (e) { console.warn("aancha: discover continuation stopped", e); break; }
        const before = videos.length;
        parseLockups(page, kind, seen, videos);
        tokens = deepFind(page, "continuationCommand").map((c) => c.token).filter(Boolean);
        if (videos.length === before) break; // no new items → stop even if a token echoes
      }
    }
    if (!channelId) throw new Error("channel_id not found in ytInitialData");
    dbg(`discover: channel ${channelId}, ${videos.length} videos (${videos.filter((v) => v.kind === "stream").length} streams)`);
    return { channel_id: channelId, videos };
  };

  const harvestMeta = async (input) => {
    say(`meta: ${input.yt_id}`);
    const p = await innertube("player", { videoId: input.yt_id });
    const d = p?.videoDetails;
    const micro = p?.microformat?.playerMicroformatRenderer;
    if (!d) throw new Error("player response has no videoDetails");
    const published = micro?.publishDate || micro?.uploadDate;
    if (!published) throw new Error("no publishDate in microformat");
    return {
      yt_id: input.yt_id,
      title: (d.title || "").slice(0, 500),
      description: (d.shortDescription || "").slice(0, 20000),
      published_at: new Date(published).toISOString(),
      duration_s: +d.lengthSeconds || null,
      channel_id: d.channelId || null,
      is_live_content: !!d.isLiveContent,
      view_count: +d.viewCount || null,
      raw_player: JSON.stringify(p).slice(0, 2000000),
    };
  };

  // Fetch the player response, returning caption tracks. Public (omit) first;
  // if that yields none and we have a session, retry with it — captions are
  // sometimes withheld from anonymous player calls. Logs what each path sees.
  const playerWithCaptions = async (videoId) => {
    for (const creds of (/SAPISID=/.test(document.cookie) ? ["omit", "same-origin"] : ["omit"])) {
      const p = await innertube("player", { videoId }, creds);
      const status = p?.playabilityStatus?.status;
      const tracks = p?.captions?.playerCaptionsTracklistRenderer?.captionTracks || [];
      dbg(`captions ${videoId} [${creds}]: playability=${status}, captions=${p?.captions ? "obj" : "absent"}, tracks=${tracks.length}`,
        tracks.map((t) => `${t.languageCode}${t.kind === "asr" ? "(auto)" : ""}`));
      if (tracks.length) return { p, tracks, creds };
      if (creds === "omit") dbg(`captions ${videoId}: no tracks anonymously; player top keys:`, Object.keys(p || {}));
    }
    return { p: null, tracks: [], creds: null };
  };

  const harvestCaptions = async (input) => {
    say(`captions: ${input.yt_id}`);
    const { tracks } = await playerWithCaptions(input.yt_id);
    if (!tracks.length) { dbg(`captions ${input.yt_id}: NONE`); return { yt_id: input.yt_id, none: true }; }
    // Prefer Russian, then any; manual over ASR within the same language.
    const score = (t) => (t.languageCode?.startsWith("ru") ? 2 : 0) + (t.kind === "asr" ? 0 : 1);
    const track = tracks.sort((a, b) => score(b) - score(a))[0];
    await pace();
    const url = new URL(track.baseUrl, location.origin);
    url.searchParams.set("fmt", "json3");
    const res = await fetch(url, { credentials: "omit" });
    dbg(`captions ${input.yt_id}: timedtext ${res.status} for ${track.languageCode}${track.kind === "asr" ? "(auto)" : ""}`);
    if (!res.ok) throw new Error(`timedtext -> ${res.status}`);
    const j3 = await res.json();
    const segments = (j3.events || [])
      .filter((e) => e.segs && e.segs.some((s) => s.utf8 && s.utf8.trim()))
      .map((e) => ({
        t_ms: e.tStartMs | 0,
        d_ms: e.dDurationMs ?? null,
        text: e.segs.map((s) => s.utf8).join("").replace(/\s+/g, " ").trim().slice(0, 2000),
      }))
      .filter((s) => s.text);
    dbg(`captions ${input.yt_id}: ${segments.length} segments`);
    if (!segments.length) return { yt_id: input.yt_id, none: true };
    return {
      yt_id: input.yt_id,
      lang: track.languageCode || "und",
      source: track.kind === "asr" ? "asr" : "manual",
      segments,
    };
  };

  // --- comments & chat (written from documented structure; deep-scan based so
  // they tolerate wrapper drift. NOT yet verified live — first real run should
  // spot-check counts, like discover's lockupViewModel pass did). ---
  const ucOrNull = (s) => (/^UC[0-9A-Za-z_-]{22}$/.test(s || "") ? s : null);
  const parseCount = (s) => {
    if (!s) return null;
    const m = String(s).replace(/\s/g, "").match(/([\d.]+)\s*([KMkmКМ])?/);
    if (!m) return null;
    const scale = /[KkК]/.test(m[2] || "") ? 1e3 : /[MmМ]/.test(m[2] || "") ? 1e6 : 1;
    const n = Math.round(parseFloat(m[1]) * scale);
    return Number.isFinite(n) ? n : null;
  };
  // The "load more" token that belongs to the section itself (not a reply expander).
  const sectionNextToken = (page) => {
    for (const cir of deepFind(page, "continuationItemRenderer")) {
      const t = cir?.continuationEndpoint?.continuationCommand?.token ||
                cir?.button?.buttonRenderer?.command?.continuationCommand?.token;
      if (t) return t;
    }
    return null;
  };
  const collectComments = (page, parentId, seen, out) => {
    // Modern YouTube ships comment data as entity-payload mutations.
    for (const p of deepFind(page, "commentEntityPayload")) {
      const id = p.properties?.commentId;
      if (!id || seen.has(id)) continue;
      seen.add(id);
      out.push({
        id,
        parent_id: parentId,
        author_channel_id: ucOrNull(p.author?.channelId),
        author_name: (p.author?.displayName || "").slice(0, 200),
        text: (p.properties?.content?.content || "").slice(0, 10000),
        like_count: parseCount(p.toolbar?.likeCountNotliked),
        published_at: null, // only relative text is exposed here; harvest doesn't need it
      });
    }
  };

  const harvestComments = async (input) => {
    say(`comments: ${input.yt_id}`);
    const first = await innertube("next", { videoId: input.yt_id });
    const section = deepFind(first, "itemSectionRenderer")
      .find((s) => s.sectionIdentifier === "comment-item-section");
    let token = section
      ? deepFind(section, "continuationCommand").map((c) => c.token).filter(Boolean)[0]
      : null;
    if (!token) return { yt_id: input.yt_id, disabled: true, comments: [] };

    const comments = [], seen = new Set(), replyThreads = [];
    let guard = 0;
    while (token && !window.AANCHA_STOP && guard++ < 100000) {
      let page;
      try { page = await innertube("next", { continuation: token }); }
      catch (e) { console.warn("aancha: comments continuation stopped", e); break; }
      collectComments(page, null, seen, comments);
      for (const thread of deepFind(page, "commentThreadRenderer")) {
        const parentId = deepFind(thread, "commentId")[0] ||
          deepFind(thread, "commentEntityPayload")[0]?.properties?.commentId;
        const rt = deepFind(thread.replies || {}, "continuationCommand").map((c) => c.token).filter(Boolean)[0];
        if (rt && parentId) replyThreads.push([rt, parentId]);
      }
      token = sectionNextToken(page);
      say(`comments: ${input.yt_id} — ${seen.size}`);
    }
    // Expand reply threads: the professor's answers live in replies (SPEC §6).
    for (const [rt, parentId] of replyThreads) {
      if (window.AANCHA_STOP) break;
      let t = rt, g2 = 0;
      while (t && g2++ < 1000) {
        let page;
        try { page = await innertube("next", { continuation: t }); }
        catch (e) { console.warn("aancha: reply continuation stopped", e); break; }
        collectComments(page, parentId, seen, comments);
        t = sectionNextToken(page);
      }
    }
    dbg(`comments ${input.yt_id}: ${comments.length} (${comments.filter((c) => c.author_channel_id).length} with channel id)`);
    return { yt_id: input.yt_id, comments };
  };

  const harvestChat = async (input) => {
    say(`chat: ${input.yt_id}`);
    // The initial replay continuation lives in the watch page's ytInitialData.
    const html = await fetchPage(`/watch?v=${input.yt_id}`);
    const data = extractJson(html, "ytInitialData");
    let cont = deepFind(data, "liveChatReplayContinuationData").map((c) => c.continuation).filter(Boolean)[0] ||
               deepFind(data, "reloadContinuationData").map((c) => c.continuation).filter(Boolean)[0];
    if (!cont) return { yt_id: input.yt_id, unavailable: true, messages: [] };

    const messages = [], seen = new Set();
    let guard = 0;
    while (cont && !window.AANCHA_STOP && guard++ < 200000) {
      let page;
      try { page = await innertube("live_chat/get_live_chat_replay", { continuation: cont }); }
      catch (e) { console.warn("aancha: chat continuation stopped", e); break; }
      const lcc = deepFind(page, "liveChatContinuation")[0];
      if (!lcc) break;
      for (const action of lcc.actions || []) {
        const offset = action.replayChatItemAction?.videoOffsetTimeMsec;
        for (const inner of action.replayChatItemAction?.actions || [action]) {
          const r = inner.addChatItemAction?.item?.liveChatTextMessageRenderer;
          if (!r || !r.id || seen.has(r.id)) continue;
          seen.add(r.id);
          messages.push({
            id: r.id,
            offset_ms: offset ? parseInt(offset, 10) : null,
            author_channel_id: ucOrNull(r.authorExternalChannelId),
            author_name: (r.authorName?.simpleText || "").slice(0, 200),
            text: (r.message?.runs || []).map((x) => x.text || "").join("").slice(0, 10000),
          });
        }
      }
      const next = deepFind(lcc, "liveChatReplayContinuationData").map((c) => c.continuation).filter(Boolean)[0];
      if (!next || next === cont) break;
      cont = next;
      if (guard % 20 === 0) say(`chat: ${input.yt_id} — ${seen.size}`);
    }
    dbg(`chat ${input.yt_id}: ${messages.length} messages`);
    return { yt_id: input.yt_id, messages };
  };

  const HANDLERS = {
    discover, harvest_meta: harvestMeta, harvest_captions: harvestCaptions,
    harvest_comments: harvestComments, harvest_chat: harvestChat,
  };

  // ---- main loop ------------------------------------------------------------
  (async () => {
    let done = 0, failed = 0;
    try {
      for (;;) {
        if (window.AANCHA_STOP) break;
        const { tasks } = await api("/api/tasks?limit=5");
        if (!tasks.length) break;
        for (const t of tasks) {
          if (window.AANCHA_STOP) break;
          dbg(`task #${t.id} ${t.type} ${t.subject}`);
          try {
            const result = await HANDLERS[t.type](t.input);
            const summary = await api(`/api/tasks/${t.id}/result`, { method: "POST", body: JSON.stringify(result) });
            dbg(`task #${t.id} ${t.type} OK →`, summary);
            done++;
          } catch (e) {
            dwarn(`task #${t.id} ${t.type} FAILED:`, e);
            await api(`/api/tasks/${t.id}/fail`, {
              method: "POST",
              body: JSON.stringify({ error: String(e).slice(0, 500) }),
            }).catch(() => {});
            failed++;
          }
          say(`done ${done}, failed ${failed} — working…`);
        }
      }
      say(`finished: ${done} done, ${failed} failed. Safe to close.`);
    } catch (e) {
      console.error("aancha collector stopped", e);
      say(`ERROR: ${String(e).slice(0, 300)}`);
    } finally {
      window.AANCHA_RUNNING = false;
      stopBtn.textContent = "close";
      stopBtn.onclick = () => box.remove();
    }
  })();
})();
