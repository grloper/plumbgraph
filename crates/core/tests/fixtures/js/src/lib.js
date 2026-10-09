export function used() {
  return helper();
}

function helper() {
  return 1;
}

export class Widget {
  constructor() {
    this.n = 0;
  }
  render() {
    return this.n;
  }
  unusedMethod() {
    return 2;
  }
}

function a() {
  return b();
}

function b() {
  return a();
}
