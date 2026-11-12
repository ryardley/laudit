import { helper } from "./helper.js";

export class Counter {
  constructor() {
    this.value = 0;
  }

  increment() {
    this.value = helper(this.value);
  }
}

export function makeCounter() {
  return new Counter();
}

export const MAX = 10;
export const double = (n) => n * 2;
