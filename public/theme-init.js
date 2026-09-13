(() => {
  let preference = "system";
  try {
    const saved = localStorage.getItem("filem:theme:v1");
    if (saved === "light" || saved === "dark") preference = saved;
  } catch {
    // The theme still works for this session when storage is unavailable.
  }
  const theme =
    preference === "system"
      ? matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : preference;
  document.documentElement.dataset.themePreference = preference;
  document.documentElement.dataset.theme = theme;
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute("content", theme === "dark" ? "#181e21" : "#f5f7f6");
})();
