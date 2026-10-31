// The welcome screen uses the same native actions as the library toolbar.
document.querySelectorAll('[data-library-action]').forEach(button => {
  button.addEventListener('click', () => {
    const action = document.getElementById(button.dataset.libraryAction);
    if (action && !action.disabled) action.click();
  });
});
