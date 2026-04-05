// Benchmarks for the Go porcupine linearizability checker.
//
// Drop this file into the root of the cloned Go porcupine repo:
//
//   git clone https://github.com/anishathalye/porcupine /tmp/porcupine-go
//   cp porcupine_bench_test.go /tmp/porcupine-go/
//
// Then symlink (or copy) the shared test data so both checkers use identical
// byte-for-byte inputs:
//
//   ln -s /path/to/porcupine-rust/test_data /tmp/porcupine-go/test_data
//
// Run benchmarks:
//
//   cd /tmp/porcupine-go
//   go test -bench=. -benchmem -count=10 -run='^$' .
//
// Apples-to-apples mapping to the Rust Criterion groups:
//
//   BenchmarkEtcdSingleFile  ↔  etcd_sequential/single_file  (use 1 thread on Rust side)
//   BenchmarkEtcdAllFiles    ↔  etcd_sequential/all_files     (use 1 thread on Rust side)
//   BenchmarkKvC10Ok         ↔  kv_partitioned/c10_ok_seq     (use 1 thread on Rust side)
//   BenchmarkKvC10Bad        ↔  kv_partitioned/c10_bad_seq    (use 1 thread on Rust side)
//
// Go's CheckOperations is single-threaded by default, which matches the Rust
// "sequential" benchmark groups (1 rayon thread).

package porcupine

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

// ---------------------------------------------------------------------------
// Etcd model  (mirrors Go's etcdModel in porcupine_test.go)
// ---------------------------------------------------------------------------

// etcdBenchModel is a local alias so this file compiles even if the upstream
// test file's etcdModel type is unexported or named differently.
// If porcupine_test.go already exports etcdModel, replace etcdBenchModel with
// etcdModel throughout.

type etcdBenchState struct {
	exists bool
	value  int64
}

type etcdBenchInput struct {
	op   int // 0=read 1=write 2=cas
	arg1 int64
	arg2 int64
}

type etcdBenchOutput struct {
	ok      bool
	exists  bool
	value   int64
	unknown bool
}

const (
	etcdRead  = 0
	etcdWrite = 1
	etcdCas   = 2
)

var etcdBenchModel = Model{
	Init: func() interface{} {
		return etcdBenchState{exists: false, value: 0}
	},
	Step: func(state, input, output interface{}) (bool, interface{}) {
		st := state.(etcdBenchState)
		inp := input.(etcdBenchInput)
		out := output.(etcdBenchOutput)

		switch inp.op {
		case etcdRead:
			var ok bool
			if !st.exists {
				ok = !out.exists || out.unknown
			} else {
				ok = (out.exists && out.value == st.value) || out.unknown
			}
			if ok {
				return true, st
			}
			return false, nil

		case etcdWrite:
			return true, etcdBenchState{exists: true, value: inp.arg1}

		case etcdCas:
			matches := st.exists && st.value == inp.arg1
			var nextState etcdBenchState
			if matches {
				nextState = etcdBenchState{exists: true, value: inp.arg2}
			} else {
				nextState = st
			}
			ok := (matches && out.ok) || (!matches && !out.ok) || out.unknown
			if ok {
				return true, nextState
			}
			return false, nil
		}
		return false, nil
	},
	Equal: func(s1, s2 interface{}) bool {
		return s1.(etcdBenchState) == s2.(etcdBenchState)
	},
}

// ---------------------------------------------------------------------------
// Jepsen etcd log parser
// ---------------------------------------------------------------------------

func parseJepsenLogBench(path string) []Operation {
	f, err := os.Open(path)
	if err != nil {
		panic(fmt.Sprintf("open %s: %v", path, err))
	}
	defer f.Close()

	type pending struct {
		id    int
		input etcdBenchInput
	}
	pendingMap := make(map[int]pending) // process → pending call
	var ops []Operation
	var completedOps []Operation
	opID := 0

	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		line := scanner.Text()
		if !strings.Contains(line, "jepsen.util") {
			continue
		}
		parts := strings.SplitN(line, "\t", 5)
		if len(parts) < 4 {
			continue
		}
		fields := strings.Fields(parts[0])
		process, _ := strconv.Atoi(fields[len(fields)-1])
		status := parts[1]
		opStr := parts[2]
		val := strings.TrimRight(parts[3], "\r\n ")

		switch status {
		case ":invoke":
			var inp etcdBenchInput
			switch opStr {
			case ":read":
				inp = etcdBenchInput{op: etcdRead}
			case ":write":
				v, _ := strconv.ParseInt(val, 10, 64)
				inp = etcdBenchInput{op: etcdWrite, arg1: v}
			case ":cas":
				inner := strings.Trim(val, "[]")
				tokens := strings.Fields(inner)
				a1, _ := strconv.ParseInt(tokens[0], 10, 64)
				a2, _ := strconv.ParseInt(tokens[1], 10, 64)
				inp = etcdBenchInput{op: etcdCas, arg1: a1, arg2: a2}
			default:
				continue
			}
			pendingMap[process] = pending{id: opID, input: inp}
			opID++

		case ":ok", ":fail":
			p, found := pendingMap[process]
			if !found {
				continue
			}
			delete(pendingMap, process)

			if status == ":fail" && opStr == ":read" && val == ":timed-out" {
				completedOps = append(completedOps, Operation{
					Input:  p.input,
					Output: etcdBenchOutput{unknown: true},
				})
				continue
			}

			var out etcdBenchOutput
			switch opStr {
			case ":read":
				if val == "nil" {
					out = etcdBenchOutput{ok: true, exists: false}
				} else {
					v, _ := strconv.ParseInt(val, 10, 64)
					out = etcdBenchOutput{ok: true, exists: true, value: v}
				}
			case ":write":
				out = etcdBenchOutput{ok: true}
			case ":cas":
				out = etcdBenchOutput{ok: status == ":ok"}
			default:
				continue
			}
			completedOps = append(completedOps, Operation{
				Input:  p.input,
				Output: out,
			})
		}
	}
	// Timed-out ops (":info") that never got a return
	for _, p := range pendingMap {
		completedOps = append(completedOps, Operation{
			Input:  p.input,
			Output: etcdBenchOutput{unknown: true},
		})
	}
	_ = ops
	return completedOps
}

func listJepsenFiles(dir string) []string {
	var files []string
	for i := 0; i <= 102; i++ {
		p := filepath.Join(dir, fmt.Sprintf("etcd_%03d.log", i))
		if _, err := os.Stat(p); err == nil {
			files = append(files, p)
		}
	}
	return files
}

// ---------------------------------------------------------------------------
// KV model  (mirrors Go's kvModel in porcupine_test.go)
// ---------------------------------------------------------------------------

type kvBenchInput struct {
	op    string // "get" "put" "append"
	key   string
	value string
}

type kvBenchOutput struct {
	value string
}

// kvBenchModel checks linearizability per key (partitioned).
var kvBenchModel = Model{
	Init: func() interface{} { return "" },
	Step: func(state, input, output interface{}) (bool, interface{}) {
		st := state.(string)
		inp := input.(kvBenchInput)
		out := output.(kvBenchOutput)
		switch inp.op {
		case "get":
			if out.value == st {
				return true, st
			}
			return false, nil
		case "put":
			return true, inp.value
		case "append":
			return true, st + inp.value
		}
		return false, nil
	},
	Equal: func(s1, s2 interface{}) bool {
		return s1.(string) == s2.(string)
	},
	Partition: func(history []Operation) [][]Operation {
		byKey := make(map[string][]Operation)
		for _, op := range history {
			inp := op.Input.(kvBenchInput)
			byKey[inp.key] = append(byKey[inp.key], op)
		}
		partitions := make([][]Operation, 0, len(byKey))
		for _, ops := range byKey {
			partitions = append(partitions, ops)
		}
		return partitions
	},
}

// ---------------------------------------------------------------------------
// KV log parser
// ---------------------------------------------------------------------------

func parseKvLogBench(path string) []Operation {
	content, err := os.ReadFile(path)
	if err != nil {
		panic(fmt.Sprintf("read %s: %v", path, err))
	}

	type pendingEntry struct {
		input kvBenchInput
	}
	pendingMap := make(map[int]pendingEntry)
	var ops []Operation

	for _, rawLine := range strings.Split(string(content), "\n") {
		line := strings.TrimSpace(rawLine)
		if line == "" {
			continue
		}
		process := int(kvFieldInt(line, ":process "))
		typ := kvFieldToken(line, ":type ")
		f := kvFieldToken(line, ":f ")
		key := kvFieldQuoted(line, ":key \"")
		value := kvFieldValue(line)

		switch typ {
		case ":invoke":
			var op string
			switch f {
			case ":get":
				op = "get"
			case ":put":
				op = "put"
			case ":append":
				op = "append"
			default:
				panic("unknown kv op: " + f)
			}
			pendingMap[process] = pendingEntry{input: kvBenchInput{op: op, key: key, value: value}}
		case ":ok":
			p := pendingMap[process]
			delete(pendingMap, process)
			ops = append(ops, Operation{
				Input:  p.input,
				Output: kvBenchOutput{value: value},
			})
		}
	}
	return ops
}

func kvFieldInt(line, key string) int64 {
	start := strings.Index(line, key) + len(key)
	rest := line[start:]
	end := strings.IndexAny(rest, ",}")
	if end < 0 {
		end = len(rest)
	}
	v, _ := strconv.ParseInt(strings.TrimSpace(rest[:end]), 10, 64)
	return v
}

func kvFieldToken(line, key string) string {
	start := strings.Index(line, key) + len(key)
	rest := line[start:]
	end := strings.IndexAny(rest, ",}")
	if end < 0 {
		end = len(rest)
	}
	return strings.TrimSpace(rest[:end])
}

func kvFieldQuoted(line, key string) string {
	start := strings.Index(line, key) + len(key)
	rest := line[start:]
	end := strings.Index(rest, "\"")
	return rest[:end]
}

func kvFieldValue(line string) string {
	key := ":value "
	start := strings.LastIndex(line, key) + len(key)
	end := strings.LastIndex(line, "}")
	rest := strings.TrimSpace(line[start:end])
	if rest == "nil" {
		return ""
	}
	return strings.Trim(rest, "\"")
}

// ---------------------------------------------------------------------------
// BENCHMARKS
// ---------------------------------------------------------------------------

// BenchmarkEtcdSingleFile checks one etcd history (etcd_000.log).
// Comparable to Criterion's etcd_sequential/single_file.
func BenchmarkEtcdSingleFile(b *testing.B) {
	ops := parseJepsenLogBench(filepath.Join("test_data", "jepsen", "etcd_000.log"))
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		CheckOperations(etcdBenchModel, ops)
	}
}

// BenchmarkEtcdAllFiles iterates over all 102 Jepsen histories.
// Comparable to Criterion's etcd_sequential/all_files.
func BenchmarkEtcdAllFiles(b *testing.B) {
	files := listJepsenFiles(filepath.Join("test_data", "jepsen"))
	histories := make([][]Operation, 0, len(files))
	for _, f := range files {
		histories = append(histories, parseJepsenLogBench(f))
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		for _, ops := range histories {
			CheckOperations(etcdBenchModel, ops)
		}
	}
}

// BenchmarkKvC10Ok checks the 10-client linearizable KV trace.
// Comparable to Criterion's kv_partitioned/c10_ok_seq.
func BenchmarkKvC10Ok(b *testing.B) {
	ops := parseKvLogBench(filepath.Join("test_data", "kv", "c10-ok.txt"))
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		CheckOperations(kvBenchModel, ops)
	}
}

// BenchmarkKvC10Bad checks the 10-client non-linearizable KV trace.
// Comparable to Criterion's kv_partitioned/c10_bad_seq.
func BenchmarkKvC10Bad(b *testing.B) {
	ops := parseKvLogBench(filepath.Join("test_data", "kv", "c10-bad.txt"))
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		CheckOperations(kvBenchModel, ops)
	}
}
