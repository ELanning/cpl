// The CPL benchmark kernels (bench/cpl/src/bench.cpl) written as idiomatic Go: int64, slices,
// strings.Builder, and math/big for the factorial. Prints "name n result ms" for each kernel.
package main

import (
	"fmt"
	"math/big"
	"os"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"
)

func arith(n int64) int64 {
	var acc int64
	for i := int64(0); i < n; i++ {
		acc = (acc + i*i) % 1000003
	}
	return acc
}

func fib(n int64) int64 {
	if n < 2 {
		return n
	}
	return fib(n-1) + fib(n-2)
}

func collatz(n int64) int64 {
	var total int64
	for k := int64(1); k <= n; k++ {
		x := k
		for x != 1 {
			if x%2 == 0 {
				x /= 2
			} else {
				x = 3*x + 1
			}
			total++
		}
	}
	return total
}

func listIndex(n int64) int64 {
	xs := make([]int64, 0)
	for i := int64(0); i < n; i++ {
		xs = append(xs, i*3%17)
	}
	var sum int64
	for _, x := range xs {
		sum += x
	}
	return sum
}

func listWalk(n int64) int64 {
	xs := make([]int64, 0)
	for i := int64(0); i < n; i++ {
		xs = append(xs, i%11)
	}
	var sum int64
	for rest := xs; len(rest) > 0; rest = rest[1:] {
		sum += rest[0]
	}
	return sum
}

func merge(a, b []int64) []int64 {
	out := make([]int64, 0, len(a)+len(b))
	i, j := 0, 0
	for i < len(a) || j < len(b) {
		if j >= len(b) || (i < len(a) && a[i] <= b[j]) {
			out = append(out, a[i])
			i++
		} else {
			out = append(out, b[j])
			j++
		}
	}
	return out
}

func sortList(xs []int64) []int64 {
	if len(xs) <= 1 {
		return xs
	}
	half := len(xs) / 2
	return merge(sortList(xs[:half]), sortList(xs[half:]))
}

func sorting(n int64) int64 {
	seed := int64(12345)
	xs := make([]int64, 0, n)
	for i := int64(0); i < n; i++ {
		seed = (seed*1103515245 + 12345) % 2147483648
		xs = append(xs, seed%100000)
	}
	var sum int64
	for _, x := range sortList(xs) {
		sum = (sum*31 + x) % 1000000007
	}
	return sum
}

type point struct{ x, y int64 }

func step(p point, k int64) point { return point{p.y%1000 + k, p.x%1000 + 2*k} }

func structs(n int64) int64 {
	p := point{1, 2}
	for i := int64(0); i < n; i++ {
		p = step(p, i%7)
	}
	return p.x + p.y
}

func text(n int64) int64 {
	var b strings.Builder
	for i := int64(0); i < n; i++ {
		b.WriteByte('w')
		b.WriteString(strconv.FormatInt(i%100, 10))
		b.WriteByte(' ')
	}
	t := b.String()
	var total int64
	for _, w := range strings.Split(t, " ") {
		total += int64(utf8.RuneCountInString(w))
	}
	return total + int64(utf8.RuneCountInString(t))
}

func bigint(n int64) int64 {
	f := big.NewInt(1)
	for i := int64(2); i <= n; i++ {
		f.Mul(f, big.NewInt(i))
	}
	return int64(len(f.String()))
}

type tree struct {
	left, right *tree
}

func makeTree(d int64) *tree {
	if d == 0 {
		return &tree{}
	}
	return &tree{makeTree(d - 1), makeTree(d - 1)}
}

func countTree(t *tree) int64 {
	if t == nil {
		return 0
	}
	return 1 + countTree(t.left) + countTree(t.right)
}

func trees(n int64) int64 {
	var total int64
	for i := int64(0); i < n; i++ {
		total += countTree(makeTree(16))
	}
	return total
}

type pt struct{ x, y int64 }

func records(n int64) int64 {
	ps := make([]pt, 0)
	for i := int64(0); i < n; i++ {
		ps = append(ps, pt{i % 1000, i % 7})
	}
	var s int64
	for _, p := range ps {
		s += p.x * p.y
	}
	return s
}

type item struct {
	sku   string
	price int64
	qty   int64
}

type disc struct {
	kind int // 0 none, 1 percent, 2 buy n get one
	v    int64
}

//go:noinline
func itemTotal(it *item, d disc) int64 {
	gross := it.price * it.qty
	switch d.kind {
	case 1:
		return gross - gross*d.v/100
	case 2:
		if d.v <= 0 {
			return gross
		}
		return gross - it.qty/(d.v+1)*it.price
	}
	return gross
}

func business(n int64) int64 {
	items := make([]item, 0, 1000)
	for i := int64(0); i < 1000; i++ {
		items = append(items, item{sku: "sku" + strconv.FormatInt(i, 10), price: 100 + i%50, qty: 1 + i%5})
	}
	var total int64
	for r := int64(0); r < n; r++ {
		for j := range items {
			jj := int64(j)
			var d disc
			if jj%3 == 1 {
				d = disc{1, jj % 30}
			} else if jj%3 == 2 {
				d = disc{2, jj % 4}
			}
			total = (total + itemTotal(&items[j], d)) % 1000000007
		}
	}
	return total
}

func matrix(n int64) int64 {
	a := make([][]int64, n)
	for i := int64(0); i < n; i++ {
		row := make([]int64, 0)
		for j := int64(0); j < n; j++ {
			row = append(row, (i*j+1)%10)
		}
		a[i] = row
	}
	var sum int64
	for i := int64(0); i < n; i++ {
		for j := int64(0); j < n; j++ {
			var acc int64
			for k := int64(0); k < n; k++ {
				acc += a[i][k] * a[k][j]
			}
			sum = (sum + acc) % 1000000007
		}
	}
	return sum
}

func main() {
	kernels := []struct {
		name string
		f    func(int64) int64
		n    int64
	}{
		{"arith", arith, 50_000_000},
		{"fib", fib, 32},
		{"collatz", collatz, 300_000},
		{"list_index", listIndex, 5_000_000},
		{"list_walk", listWalk, 5_000_000},
		{"sorting", sorting, 500_000},
		{"structs", structs, 20_000_000},
		{"text", text, 1_000_000},
		{"bigint", bigint, 5_000},
		{"trees", trees, 20},
		{"records", records, 2_000_000},
		{"matrix", matrix, 150},
		{"business", business, 2000},
	}
	only := map[string]bool{}
	for _, a := range os.Args[1:] {
		only[a] = true
	}
	for _, k := range kernels {
		if len(only) > 0 && !only[k.name] {
			continue
		}
		best := 1e18
		var result int64
		for r := 0; r < 3; r++ {
			t := time.Now()
			result = k.f(k.n)
			if ms := float64(time.Since(t).Nanoseconds()) / 1e6; ms < best {
				best = ms
			}
		}
		fmt.Printf("%s %d %d %.2f\n", k.name, k.n, result, best)
	}
}
