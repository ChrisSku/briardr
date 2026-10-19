// Cancel native selection on controls with a custom hold gesture, without cancelling touch/scroll.
export function preventSelection(node: HTMLElement) {
  const prevent = (event: Event) => event.preventDefault();
  node.addEventListener('selectstart', prevent);
  return { destroy: () => node.removeEventListener('selectstart', prevent) };
}
