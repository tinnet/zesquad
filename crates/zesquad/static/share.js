// The squad page's Copy buttons: each copies the link from the field next to it. Without script the
// button stays hidden and the field can still be selected and copied by hand.
document.querySelectorAll("[data-copy]").forEach((button) => {
  const field = document.getElementById(button.dataset.copy);
  if (!field || !navigator.clipboard) return;
  const label = button.textContent;
  button.hidden = false;
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(field.value);
      button.textContent = "Copied";
    } catch {
      field.select();
      button.textContent = "Press Ctrl+C";
    }
    setTimeout(() => (button.textContent = label), 2000);
  });
});
