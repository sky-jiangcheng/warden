/* Apply the stored theme before first paint so a dark-session reload does
   not flash light. main.ts owns the three-state preference; this only mirrors
   its resolution early enough to avoid the flash. A synchronous classic
   script runs before the first paint; a module script (defer) would not. */
(function () {
  if (new URLSearchParams(location.search).get("surface") === "companion") {
    document.documentElement.dataset.surface = "companion";
  }
  try {
    var pref = localStorage.getItem("warden-theme") || "system";
    var dark =
      pref === "dark" ||
      (pref === "system" &&
        window.matchMedia("(prefers-color-scheme: dark)").matches);
    if (dark) document.documentElement.setAttribute("data-theme", "dark");
  } catch (e) {
    /* Private mode: fall through to the light default. */
  }
})();
