// Light or dark, decided before the page first paints: this device's choice (Settings → Appearance,
// kept in briardr.prefs), else the system's. The styles follow data-theme on <html>, and so does
// whatever draws its own colours (the terminal, diagrams; web/src/theme.ts). The build puts this
// script into the page itself, ahead of the styles: the app's modules run after the first paint.
(function () {
  var system = matchMedia('(prefers-color-scheme: dark)');
  function apply() {
    var choice;
    try {
      choice = (JSON.parse(localStorage.getItem('briardr.prefs') || '{}') || {}).theme;
    } catch (e) {}
    var dark = choice === 'dark' || (choice !== 'light' && system.matches);
    document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
    var bar = document.querySelector('meta[name="theme-color"]');
    if (bar) bar.setAttribute('content', dark ? '#0d0c0b' : '#f6f5f3');
  }
  apply();
  system.addEventListener('change', apply);
  // The choice changed: in another window (storage), or in this one (the app says so).
  addEventListener('storage', apply);
  addEventListener('briardr:theme', apply);
})();
