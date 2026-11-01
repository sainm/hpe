const fs = require("node:fs");
const path = require("node:path");
const Ajv2020Module = require("ajv/dist/2020");
const addFormats = require("ajv-formats");
const YAML = require("yaml");

const Ajv2020 = Ajv2020Module.default || Ajv2020Module;
const root = path.resolve(__dirname, "..");
const schemaPath = path.join(root, "schema", "ir", "v0.1", "workflow.schema.json");
const fixturesDir = path.join(root, "fixtures");
const errors = [];

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.keys(value)
      .sort()
      .reduce((out, key) => {
        out[key] = canonical(value[key]);
        return out;
      }, {});
  }
  return value;
}

function walkGraph(graph, label, workflow, seenSubflows) {
  if (!graph || !Array.isArray(graph.nodes) || !Array.isArray(graph.edges)) {
    errors.push(`${label}: graph must contain nodes and edges`);
    return;
  }
  const ids = new Set();
  for (const node of graph.nodes) {
    if (ids.has(node.id)) errors.push(`${label}: duplicate node id ${node.id}`);
    ids.add(node.id);
  }
  for (const edge of graph.edges) {
    if (!ids.has(edge.source)) errors.push(`${label}: edge ${edge.id} has unknown source ${edge.source}`);
    if (!ids.has(edge.target)) errors.push(`${label}: edge ${edge.id} has unknown target ${edge.target}`);
  }
  for (const node of graph.nodes) {
    if (node.type === "loop") {
      if (!node.body) errors.push(`${label}: loop ${node.id} missing body`);
      else walkGraph(node.body, `${label}/${node.id}`, workflow, seenSubflows);
    }
    if (node.type === "parallel") {
      if (!Array.isArray(node.branches) || node.branches.length === 0) {
        errors.push(`${label}: parallel ${node.id} missing branches`);
      } else {
        for (const branch of node.branches) {
          walkGraph(branch.graph, `${label}/${node.id}/${branch.id}`, workflow, seenSubflows);
        }
      }
    }
    if (node.type === "subflow") {
      const subflowId = node.config && node.config.subflowId;
      if (!subflowId) errors.push(`${label}: subflow ${node.id} missing config.subflowId`);
      else if (!seenSubflows.has(subflowId)) errors.push(`${label}: subflow ${node.id} references unknown ${subflowId}`);
    }
  }
}

function validateFile(file) {
  const raw = fs.readFileSync(file, "utf8");
  let doc;
  try {
    doc = JSON.parse(raw);
  } catch (error) {
    errors.push(`${path.relative(root, file)}: invalid JSON: ${error.message}`);
    return;
  }
  if (!validate(doc)) {
    for (const issue of validate.errors || []) {
      errors.push(`${path.relative(root, file)}: ${issue.instancePath || "/"} ${issue.message}`);
    }
    return;
  }
  const subflowIds = new Set((doc.subflows || []).map((item) => item.id));
  walkGraph({ nodes: doc.nodes, edges: doc.edges }, doc.id, doc, subflowIds);
  for (const subflow of doc.subflows || []) {
    walkGraph(subflow.graph, `${doc.id}/${subflow.id}`, doc, subflowIds);
  }
}

const schema = JSON.parse(fs.readFileSync(schemaPath, "utf8"));
const ajv = new Ajv2020({ allErrors: true, strict: false });
addFormats(ajv);
const validate = ajv.compile(schema);

const jsonFixtures = fs
  .readdirSync(fixturesDir)
  .filter((name) => name.endsWith(".json"))
  .sort();

if (jsonFixtures.length !== 10) {
  errors.push(`expected 10 JSON fixtures, found ${jsonFixtures.length}`);
}

for (const name of jsonFixtures) validateFile(path.join(fixturesDir, name));

const yamlFixture = path.join(fixturesDir, "01_sequence.yaml");
const yamlJson = path.join(fixturesDir, "01_sequence.json");
if (fs.existsSync(yamlFixture) && fs.existsSync(yamlJson)) {
  const fromYaml = YAML.parse(fs.readFileSync(yamlFixture, "utf8"));
  const fromJson = JSON.parse(fs.readFileSync(yamlJson, "utf8"));
  if (JSON.stringify(canonical(fromYaml)) !== JSON.stringify(canonical(fromJson))) {
    errors.push("01_sequence.yaml does not match 01_sequence.json");
  }
} else {
  errors.push("01_sequence.yaml or 01_sequence.json missing");
}

if (errors.length) {
  console.error(`IR check failed (${errors.length}):`);
  for (const error of errors) console.error(`- ${error}`);
  process.exitCode = 1;
} else {
  console.log(`IR check passed: ${jsonFixtures.length} JSON fixtures, 1 YAML roundtrip`);
}
