import type { InsightRule } from "./api";

/**
 * What each check is about, independent of any one capture.
 *
 * A finding's own text says what was seen *this time* and what to do; this is
 * the background behind the ⓘ — what the rule looks for, why it is worth
 * fixing, and what it costs left alone. A `Record` over every rule, so adding a
 * rule in the engine without writing its entry here fails the type check.
 */
export interface InsightDoc {
  /** What the check looks for, in one sentence. */
  what: string;
  why: string;
  fix: string;
  /** What it costs, in the terms someone deciding whether to fix it uses. */
  impact: string;
}

export const INSIGHT_DOCS: Record<InsightRule, InsightDoc> = {
  burst: {
    what: "The same endpoint called five or more times within two seconds by one app.",
    why: "Run one after another it is an N+1 loop: the total wait is the sum of every call. Run at once it is a fan-out that one batched request could replace.",
    fix: "Batch the ids into one request, de-duplicate identical in-flight requests, or cache the result.",
    impact: "Latency, server load",
  },
  polling: {
    what: "GETs to one endpoint at a steady interval between one second and five minutes.",
    why: "Most polls return nothing new, yet each one costs a round trip, battery on mobile, and server capacity that grows with every open client.",
    fix: "Push changes (SSE or WebSocket), back off while nothing changes, or make each poll conditional with ETag / If-None-Match.",
    impact: "Battery, bandwidth, server load",
  },
  "retry-storm": {
    what: "Three or more failures of one endpoint within ten seconds.",
    why: "Retrying a struggling server quickly adds load exactly when it can least take it, and turns a short outage into a long one.",
    fix: "Exponential backoff with jitter, honour Retry-After, and stop after a few attempts (circuit breaker).",
    impact: "Outage length, server load",
  },
  "rate-limited": {
    what: "429 Too Many Requests responses, and whether they said when to retry.",
    why: "The client is sending more than the server will accept; without Retry-After it can only guess how long to wait.",
    fix: "Cache, batch or space out the calls; servers should send Retry-After with every 429.",
    impact: "Failed requests, degraded features",
  },
  "error-rate": {
    what: "A quarter or more of one app's requests to a host failing (at least ten requests).",
    why: "A high failure share usually hides one repeating cause — an expired token, a stale URL, a broken deploy.",
    fix: "Open the failing flows and group them by status: 401/403 point at auth, 404 at URLs, 5xx at the server.",
    impact: "Broken features",
  },
  uncompressed: {
    what: "Text responses of 10 KB or more (JSON, HTML, JS, CSS, SVG) sent without gzip or brotli.",
    why: "Text compresses 70–90%; sending it raw costs download time on every slow or metered connection.",
    fix: "Enable gzip or brotli for text types on the server or CDN.",
    impact: "Load time, bandwidth",
  },
  "large-payload": {
    what: "JSON or XML responses of 1 MB or more, or HTML pages of 500 KB or more.",
    why: "Beyond the download, the client has to parse all of it — on a phone that blocks the main thread for a noticeable time.",
    fix: "Paginate, let the client choose fields, stream the result, or move inlined data out of HTML into cacheable files.",
    impact: "Load time, memory, responsiveness",
  },
  "static-uncacheable": {
    what: "Scripts, styles, fonts and images marked no-store, or sent with no caching headers at all.",
    why: "The browser has to download them again on every visit, though they rarely change.",
    fix: "Long max-age plus immutable for fingerprinted files; an ETag for the rest so revisits cost a 304.",
    impact: "Repeat-visit load time, bandwidth",
  },
  "short-cache": {
    what: "Assets with a build hash in their file name cached for less than a week.",
    why: "A hashed file can never go stale — a new build ships new names — so a short lifetime only throws the cache away.",
    fix: "Cache-Control: public, max-age=31536000, immutable for hashed assets.",
    impact: "Repeat-visit load time",
  },
  "redirect-chain": {
    what: "Three or more redirects before a request gets its answer.",
    why: "Every hop is a full round trip — on mobile networks each one can add hundreds of milliseconds before anything loads.",
    fix: "Link to the final URL, and collapse http→https, www and trailing-slash redirects into one.",
    impact: "Load time",
  },
  "redirect-loop": {
    what: "Redirects that come back to a URL already visited.",
    why: "The client never gets an answer; browsers give up with an error after about twenty hops.",
    fix: "Check what each hop redirects on — usually a cookie or auth check that never succeeds.",
    impact: "Page does not load",
  },
  "slow-endpoint": {
    what: "Median server wait of a second or more over three calls, or any single call waiting ten seconds.",
    why: "This is time before the first byte with connection setup excluded — the server itself is slow, not the network.",
    fix: "Profile the queries or upstream calls behind the endpoint, cache its result, or make slow work asynchronous (202 and poll).",
    impact: "Latency, timeouts",
  },
  "cors-preflight": {
    what: "Five or more CORS preflight (OPTIONS) requests to one host.",
    why: "Each preflight is an extra round trip before the real request; without Access-Control-Max-Age browsers keep the answer for only a few seconds.",
    fix: "Send Access-Control-Max-Age (e.g. 7200), avoid headers or content types that force preflights, or serve the API from the same origin.",
    impact: "Latency",
  },
  "token-in-url": {
    what: "Tokens, keys, passwords or JWTs in query strings.",
    why: "URLs are written to server, proxy and CDN logs, browser history, and leak to other sites through the Referer header.",
    fix: "Send credentials in the Authorization header or the request body.",
    impact: "Credential leak",
  },
  "insecure-cookie": {
    what: "Cookies set over HTTPS without Secure, or session cookies without HttpOnly.",
    why: "Without Secure a cookie can be sent over plain HTTP and read on the network; without HttpOnly any injected script can steal the session.",
    fix: "Mark every cookie Secure, and session and auth cookies HttpOnly too.",
    impact: "Session hijacking",
  },
  "plaintext-credentials": {
    what: "Authorization headers, API keys or cookies sent over plain HTTP.",
    why: "Anyone on the network path — public Wi-Fi, a compromised router — can read and reuse them.",
    fix: "Serve everything that carries a credential over HTTPS, and send HSTS.",
    impact: "Credential theft",
  },
  "image-unoptimized": {
    what: "JPEG, PNG or GIF images of 100 KB or more, or any image of 500 KB or more.",
    why: "Images are usually the heaviest bytes on a page; modern formats and right-sized images cut them substantially.",
    fix: "Serve WebP or AVIF, size images to how they are displayed (srcset), and lazy-load below the fold.",
    impact: "Load time, bandwidth",
  },
  "mixed-content": {
    what: "Plain-HTTP requests made from an HTTPS page.",
    why: "Browsers block scripts and styles loaded this way and warn about the rest; whatever loads can be read or altered on the way.",
    fix: "Load the resource over HTTPS, and add Content-Security-Policy: upgrade-insecure-requests.",
    impact: "Broken page, tampering",
  },
  "missing-security-headers": {
    what: "HTML pages without HSTS, a Content-Security-Policy, nosniff, or framing protection.",
    why: "These headers are the cheap, browser-enforced defences against downgrade attacks, script injection, MIME confusion and clickjacking.",
    fix: "Send Strict-Transport-Security, a Content-Security-Policy, X-Content-Type-Options: nosniff, and frame-ancestors (or X-Frame-Options).",
    impact: "Security hardening",
  },
  "broken-asset": {
    what: "404 or 410 responses for scripts, styles, fonts or images.",
    why: "A missing script or stylesheet breaks how the page works, not only how it looks — and it is often invisible until a user reports it.",
    fix: "Fix the reference or deploy the file; after a release, keep the previous build's hashed assets around for a while.",
    impact: "Broken page",
  },
  "empty-response": {
    what: "An endpoint answering 200 with an empty body (or an empty JSON value) three or more times.",
    why: "Either the call is not needed at all, or it is failing without saying so — both are worth knowing.",
    fix: "Answer 204 when nothing is expected; skip or make conditional calls that check for news; return an error status when data was expected.",
    impact: "Wasted requests, silent failures",
  },
  "slow-connect": {
    what: "Median DNS over 200 ms, TCP connect over 300 ms, or TLS over 500 ms to one host.",
    why: "Connection setup happens before the first request byte; when it is slow every new connection pays it, and pages open several.",
    fix: "Reuse connections (keep-alive, HTTP/2), preconnect hosts the page needs, enable TLS 1.3 and session resumption, or move behind a CDN.",
    impact: "Latency on new connections",
  },
  "cdn-miss": {
    what: "Static assets behind a CDN where half or more are not served from its cache.",
    why: "A CDN that misses is just an extra hop: the origin still serves every byte, from farther away.",
    fix: "Check for Cache-Control: private or no-cache from the origin, cookies or query strings in the cache key, or a broad Vary.",
    impact: "Load time, origin load",
  },
};
