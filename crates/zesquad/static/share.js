// The squad page's Copy button: copies the link from the field next to it. Without script the
// button stays hidden and the field can still be selected and copied by hand.
document.querySelectorAll("[data-copy]").forEach((button) => {
  const field = document.getElementById(button.dataset.copy);
  if (!field || !navigator.clipboard) return;
  button.hidden = false;
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(field.value);
      button.textContent = "Copied";
    } catch {
      field.select();
      button.textContent = "Press Ctrl+C";
    }
    setTimeout(() => (button.textContent = "Copy link"), 2000);
  });
});
