"""Build the .onnx fixtures `skynet-ort`'s suite runs against.

Committed alongside the models it produces, because a fixture whose provenance is "somebody
generated it once" is a fixture nobody can check. Re-run with:

    python crates/skynet-ort/tests/make-fixtures.py

Needs `onnx` (`pip install onnx`) and nothing else. Deliberately does **not** need onnxruntime:
generating a graph and running one are separate jobs, and the suite is what runs them.

Every model here has an output that is arithmetic a test can predict, so a passing test means the
numbers came back right rather than merely came back.
"""

import os

import onnx
from onnx import TensorProto, helper, numpy_helper
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "fixtures")
os.makedirs(OUT, exist_ok=True)


def save(model, name):
    onnx.checker.check_model(model)
    path = os.path.join(OUT, name)
    onnx.save(model, path)
    print(f"{name}: {os.path.getsize(path)} bytes")


def affine(name, weights, bias, *, batch_dim, opset=13):
    """y = x @ W + b, as a single Gemm. `batch_dim` is 1 or None (symbolic)."""
    rows, cols = weights.shape  # inputs x outputs
    batch = batch_dim if batch_dim is not None else "batch"
    graph = helper.make_graph(
        nodes=[helper.make_node("Gemm", ["observation", "W", "b"], ["setpoint"])],
        name=name.replace(".onnx", ""),
        inputs=[helper.make_tensor_value_info("observation", TensorProto.FLOAT, [batch, rows])],
        outputs=[helper.make_tensor_value_info("setpoint", TensorProto.FLOAT, [batch, cols])],
        initializer=[
            numpy_helper.from_array(weights.astype(np.float32), "W"),
            numpy_helper.from_array(bias.astype(np.float32), "b"),
        ],
    )
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", opset)])
    model.ir_version = 9
    save(model, name)


# 1. The workhorse: 4 floats in (2 channels x position+velocity), 2 setpoints out.
#    Chosen so each output is a distinct, easily-predicted combination:
#      setpoint[0] = 1*in0 + 0*in1 + 0*in2 + 0*in3 + 0.5
#      setpoint[1] = 0*in0 + 2*in1 + 0*in2 + 0*in3 - 1.0
affine(
    "affine-4-2.onnx",
    weights=np.array(
        [[1.0, 0.0],
         [0.0, 2.0],
         [0.0, 0.0],
         [0.0, 0.0]]
    ),
    bias=np.array([0.5, -1.0]),
    batch_dim=1,
)

# 2. The same arithmetic with a symbolic batch, which is what a real export produces. Accepted, and
#    the test asserts the width is still read off the last dimension.
affine(
    "affine-4-2-dynamic-batch.onnx",
    weights=np.array(
        [[1.0, 0.0],
         [0.0, 2.0],
         [0.0, 0.0],
         [0.0, 0.0]]
    ),
    bias=np.array([0.5, -1.0]),
    batch_dim=None,
)

# 3. A model whose input width is 1 and output width 1, for the vision-pooled layout test:
#    1 channel reading position only, plus a 2x2 pooled frame = 5 in. Refused-width control.
affine(
    "affine-5-1.onnx",
    weights=np.array([[1.0], [1.0], [1.0], [1.0], [1.0]]),
    bias=np.array([0.0]),
    batch_dim=1,
)

# 4. float64 input — refused by name rather than reinterpreted.
rows, cols = 4, 2
graph = helper.make_graph(
    nodes=[helper.make_node("Gemm", ["observation", "W", "b"], ["setpoint"])],
    name="double-input",
    inputs=[helper.make_tensor_value_info("observation", TensorProto.DOUBLE, [1, rows])],
    outputs=[helper.make_tensor_value_info("setpoint", TensorProto.DOUBLE, [1, cols])],
    initializer=[
        numpy_helper.from_array(np.zeros((rows, cols), dtype=np.float64), "W"),
        numpy_helper.from_array(np.zeros(cols, dtype=np.float64), "b"),
    ],
)
model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
model.ir_version = 9
save(model, "double-input.onnx")

# 5. Two inputs — refused, because which one is the observation is not this seam's to guess.
graph = helper.make_graph(
    nodes=[helper.make_node("Add", ["observation", "extra"], ["setpoint"])],
    name="two-inputs",
    inputs=[
        helper.make_tensor_value_info("observation", TensorProto.FLOAT, [1, 4]),
        helper.make_tensor_value_info("extra", TensorProto.FLOAT, [1, 4]),
    ],
    outputs=[helper.make_tensor_value_info("setpoint", TensorProto.FLOAT, [1, 4])],
)
model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
model.ir_version = 9
save(model, "two-inputs.onnx")

# 6. A dynamic *last* dimension — refused, because a width that accepts anything can be checked
#    against no body. Contrast with fixture 2, where the dynamic dimension is the batch.
graph = helper.make_graph(
    nodes=[helper.make_node("Identity", ["observation"], ["setpoint"])],
    name="dynamic-width",
    inputs=[helper.make_tensor_value_info("observation", TensorProto.FLOAT, [1, "width"])],
    outputs=[helper.make_tensor_value_info("setpoint", TensorProto.FLOAT, [1, "width"])],
)
model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
model.ir_version = 9
save(model, "dynamic-width.onnx")
