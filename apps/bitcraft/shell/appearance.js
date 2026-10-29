// The desktop app hands over the player's theme before this runs; elsewhere
// these pages stay light.
document.documentElement.dataset.theme = window.bitcraftAppearance === "dark" ? "dark" : "light";
