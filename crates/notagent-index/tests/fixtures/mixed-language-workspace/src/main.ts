export function calculateTotal(items: number[]): number {
  return items.reduce((sum, value) => sum + value, 0);
}
