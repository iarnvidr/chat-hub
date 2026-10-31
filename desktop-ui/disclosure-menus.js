const menus = [...document.querySelectorAll('details.library-tools, details.update-menu')];

function closeMenus(except) {
  for (const menu of menus) {
    if (menu !== except) menu.open = false;
  }
}

for (const menu of menus) {
  menu.addEventListener('toggle', () => {
    if (menu.open) closeMenus(menu);
  });
}

document.addEventListener('click', event => {
  if (!menus.some(menu => menu.contains(event.target))) closeMenus();
});

document.addEventListener('keydown', event => {
  if (event.key !== 'Escape') return;
  const openMenu = menus.find(menu => menu.open);
  if (!openMenu) return;
  closeMenus();
  openMenu.querySelector(':scope > summary')?.focus();
  event.preventDefault();
});
