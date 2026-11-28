// Applies a saved colour-scheme choice before the page paints, so there is no flash of the wrong theme.
// (A separate file, not inline, because the page's content-security-policy allows scripts only from itself.)
(() => {
  try {
    const meta = document.querySelector('meta[name="color-scheme"]');
    const saved = localStorage.getItem('color-scheme');
    if (meta && (saved === 'light' || saved === 'dark')) meta.content = saved;
  } catch { /* storage blocked: follow the system setting */ }
})();
