/**
 * How a `CaStatus` reads to a human.
 *
 * Split out of the view tree because two surfaces phrase the same four trust
 * states — the Certificate page and the onboarding walkthrough — and a second
 * wording of "trusted for your login only" is a second thing to get wrong.
 */

import type { CaStatus } from "./api";

/**
 * What the user trust domain actually covers, which differs by platform: on
 * Linux there is no per-user OpenSSL store, so a user-domain install reaches
 * browsers only and `curl`/Python/Go still reject our leaf certs.
 */
export function userDomainLabel(platform: string | undefined): string {
  return platform === "linux" ? "browsers" : "this user";
}

export function trustLabel(ca: CaStatus | null): { text: string; kind: "trusted" | "untrusted" } {
  if (!ca?.trusted) return { text: "Not installed", kind: "untrusted" };
  const user = userDomainLabel(ca.platform);
  if (ca.trusted_user && ca.trusted_system) return { text: `Trusted · ${user} + all users`, kind: "trusted" };
  if (ca.trusted_system) return { text: "Trusted · all users", kind: "trusted" };
  return { text: `Trusted · ${user}`, kind: "trusted" };
}

/** How each platform describes the no-admin install and what it costs. */
export function trustHint(ca: CaStatus): string {
  if (ca.trusted && ca.trusted_system) {
    return "Trusted machine-wide: every user account, command-line tool and root-owned daemon accepts it.";
  }
  if (ca.trusted) {
    return ca.platform === "linux"
      ? "Trusted in your browser certificate databases only — no password was needed. Command-line tools (curl, Python, Go) will still reject it until you install for all users."
      : "Trusted for your login only — no administrator password was needed. Other user accounts and root-owned daemons will not accept it.";
  }
  if (ca.platform === "windows") {
    return "Installing for you writes your personal certificate store and needs no prompt at all. Choose “all users” (one UAC prompt) if other accounts or services need to trust it.";
  }
  if (ca.platform === "linux") {
    return "Installing for you adds the CA to your browser certificate databases (Chrome, Firefox) — no password needed, but command-line tools are not covered. Choose “all users” (one polkit prompt) to add a system trust anchor.";
  }
  return "Installing for your user only needs a keychain confirmation, not an administrator password. Choose “all users” if you need root-owned daemons or other accounts to trust it too.";
}
