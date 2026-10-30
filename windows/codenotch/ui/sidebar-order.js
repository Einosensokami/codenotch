// Stable ids keep a key's position when its label or budget changes.
function orderSidebarItems(items, order) {
  const ranks = new Map(order.map((id, index) => [id, index]));
  return items.slice().sort((a, b) =>
    (ranks.get(a.id) ?? Infinity) - (ranks.get(b.id) ?? Infinity));
}

function moveSidebarItem(order, items, id, direction) {
  const visible = orderSidebarItems(items, order).map(item => item.id);
  const from = visible.indexOf(id), to = from + direction;
  if (from < 0 || to < 0 || to >= visible.length) return order.slice();
  [visible[from], visible[to]] = [visible[to], visible[from]];
  // Retain temporarily unavailable accounts so they recover their saved position.
  const result = order.slice();
  for (const value of visible) if (!result.includes(value)) result.push(value);
  let at = 0;
  return result.map(value => visible.includes(value) ? visible[at++] : value);
}

function claudeSidebarItems(windows) {
  const accounts = [];
  for (const w of windows || []) {
    const name = w.group || '';
    if (!accounts.some(a => a.name === name)) {
      const slug = String(w.id).split('@')[1] || '';
      accounts.push({id: slug ? 'claude@' + slug : 'claude', name: name || 'Claude', base:'claude'});
    }
  }
  return accounts.length < 2 ? [{id:'claude', name:'Claude', base:'claude'}] : accounts;
}
