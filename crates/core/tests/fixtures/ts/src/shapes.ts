export interface Shape {
  w: number;
}

export function area(s: Shape): number {
  return s.w;
}

interface DeadShape {
  z: number;
}

type DeadAlias = string;

enum DeadEnum {
  A,
}

export class Circle {
  radius() {
    return 1;
  }
}
