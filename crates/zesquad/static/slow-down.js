// htmx doesn't swap error responses, so a change refused for coming too fast (429) would look
// like nothing happened. Say so instead.
document.addEventListener("htmx:responseError", (event) => {
  if (event.detail.xhr.status === 429) {
    alert("That was a lot in a short time. Wait a minute, then try again.");
  }
});
