export async function copyText(value: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(value);
      return true;
    }
  } catch {
    // Fall through to the legacy path.
  }
  try {
    const textarea = document.createElement('textarea');
    textarea.value = value;
    textarea.setAttribute('readonly', 'true');
    textarea.style.position = 'fixed';
    textarea.style.opacity = '0';
    document.body.appendChild(textarea);
    textarea.select();
    const success = document.execCommand('copy');
    document.body.removeChild(textarea);
    return success;
  } catch {
    return false;
  }
}

export const SENSITIVE_CLEAR_MS = 30_000;

/**
 * Copies something secret (an invite, the six words, a recovery phrase) and clears it from the clipboard after
 * thirty seconds, but only if it is still what is there (never wipe something the person copied since). Android does
 * the same. If the clipboard cannot be read back (no permission, window not focused), it is left alone.
 */
export async function copySensitive(value: string): Promise<boolean> {
  const ok = await copyText(value);
  if (ok) {
    window.setTimeout(() => {
      void (async () => {
        try {
          if ((await navigator.clipboard.readText()) === value) await navigator.clipboard.writeText('');
        } catch {
          // Cannot read it back: leave it.
        }
      })();
    }, SENSITIVE_CLEAR_MS);
  }
  return ok;
}
