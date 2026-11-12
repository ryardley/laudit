import { helper } from "./helper";

export interface Shape {
  area(): number;
}

export class Circle implements Shape {
  constructor(public radius: number) {}

  area(): number {
    return helper(this.radius);
  }
}

export function makeCircle(radius: number): Circle {
  return new Circle(radius);
}

export const PI = 3.14;
export type Radius = number;
