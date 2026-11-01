'use strict';

// Runs before the stylesheet so a saved dark theme never flashes light on launch.
(() => {
  function apply(value) {
    const theme = value === 'dark' ? 'dark' : 'light';
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
    return theme;
  }
  let saved = 'light';
  try { saved = localStorage.getItem('soulkit.theme') || saved; } catch (_) {}
  apply(saved);
  window.SoulkitTheme = {
    set(value) {
      const theme = apply(value);
      try { localStorage.setItem('soulkit.theme', theme); } catch (_) {}
    }
  };
})();
