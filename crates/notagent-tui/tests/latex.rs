//! Port of `packages/tui/test/latex.test.ts` (496 LOC).
//!
//! The five `defineCases` tables live in the generated `latex_cases` module.

mod latex_cases;

use notagent_tui::latex::{RenderLatexOptions, render_latex};

fn inline(source: &str) -> Option<String> {
    render_latex(source, RenderLatexOptions::default())
}

fn display(source: &str) -> Option<String> {
    render_latex(source, RenderLatexOptions { display: true })
}

fn check_table(cases: &[(&str, &str)]) {
    for (source, expected) in cases {
        assert_eq!(
            inline(source).as_deref(),
            Some(*expected),
            "renders {source:?}"
        );
    }
}

#[test]
fn jacobian_conjecture_session_using_dollar_delimiters() {
    check_table(latex_cases::JACOBIAN_CONJECTURE_SESSION_USING_DOLLAR_DELIMITERS);
}

#[test]
fn satellite_calculation_session_using_bracket_delimiters() {
    check_table(latex_cases::SATELLITE_CALCULATION_SESSION_USING_BRACKET_DELIMITERS);
}

#[test]
fn jacobian_conjecture_sessions_using_parenthesis_and_bracket_delimiters() {
    check_table(latex_cases::JACOBIAN_CONJECTURE_SESSIONS_USING_PARENTHESIS_AND_BRACKET_DELIMITERS);
}

#[test]
fn jacobian_matrix_session_using_dollar_delimiters() {
    check_table(latex_cases::JACOBIAN_MATRIX_SESSION_USING_DOLLAR_DELIMITERS);
}

#[test]
fn extended_formulas_from_a_renderer_stress_test_session() {
    check_table(latex_cases::EXTENDED_FORMULAS_FROM_A_RENDERER_STRESS_TEST_SESSION);
}

#[test]
fn renders_common_symbols_roots_sums_and_integrals() {
    assert_eq!(
        inline(r"\sum_{i=0}^n \alpha_i + \int_0^\infty e^{-x^2}\,dx = \sqrt{\pi}").as_deref(),
        Some("∑ᵢ₌₀ⁿ αᵢ + ∫₀^∞ e^(-x²) dx = √π")
    );
}

#[test]
fn renders_common_accents_and_binomial_notation() {
    assert_eq!(
        inline(r"\binom{n}{k}+\vec{x}+\hat{y}+\overline{AB}").as_deref(),
        Some("(n choose k)+x\u{20d7}+y\u{302}+overline(AB)")
    );
}

#[test]
fn renders_extended_symbols_and_negated_relations() {
    assert_eq!(
        inline(r"\epsilon+\varepsilon+\varsigma+\varkappa+\oplus+\otimes+\therefore+\because")
            .as_deref(),
        Some("ϵ+ε+ς+ϰ+⊕+⊗+∴+∵")
    );
    assert_eq!(
        inline(r"A\not\subseteq B,\quad x\not\in X").as_deref(),
        Some("A ⊈ B, x ∉ X")
    );
}

#[test]
fn renders_delimiter_commands_and_invisible_delimiters() {
    assert_eq!(
        inline(r"\lvert{x}\rvert+\lVert{v}\rVert+\left.\frac{dy}{dx}\right|_{x=0}").as_deref(),
        Some("|x|+‖v‖+dy/(dx)|ₓ₌₀")
    );
    assert_eq!(
        inline(r"\left\lbrace x \middle| x>0 \right\rbrace").as_deref(),
        Some("{ x | x > 0 }")
    );
}

#[test]
fn renders_named_modular_overlaid_and_underlaid_operators() {
    assert_eq!(
        inline(r"\operatorname*{arg\,max}_{x\in X} f(x)").as_deref(),
        Some("arg max[x∈X] f(x)")
    );
    assert_eq!(
        inline(r"a\bmod n,\quad a\equiv b\pmod n").as_deref(),
        Some("a mod n, a ≡ b (mod n)")
    );
    assert_eq!(
        inline(r"\overset{!}{=}+\underset{n}{x}+\stackrel{def}{=}").as_deref(),
        Some("=^!+xₙ+=ᵈᵉᶠ")
    );
}

#[test]
fn renders_indexed_roots_and_additional_accents_and_wrappers() {
    assert_eq!(
        inline(r"\sqrt[2]{x}+\sqrt[3]{x}+\sqrt[4]{x}+\sqrt[n]{x}+\sqrt[k]{x+1}").as_deref(),
        Some("√x+∛x+∜x+ⁿ√x+ᵏ√(x+1)")
    );
    assert_eq!(
        inline(r"\acute{x}+\grave{y}+\widehat{xyz}+\overrightarrow{AB}").as_deref(),
        Some("x\u{301}+y\u{300}+widehat(xyz)+overrightarrow(AB)")
    );
    assert_eq!(
        inline(r"\textnormal{hello}+\mbox{world}+\boldsymbol{x}").as_deref(),
        Some("hello+world+x")
    );
}

#[test]
fn renders_additional_display_environments() {
    assert_eq!(
        inline(r"\begin{equation}\begin{split}a&=b\\&=c\end{split}\end{equation}").as_deref(),
        Some("a = b\n= c")
    );
    assert_eq!(
        inline(r"\begin{alignedat}{2}a&=b&\quad c&=d\\e&=f&g&=h\end{alignedat}").as_deref(),
        Some("a = b c = d\ne = f g = h")
    );
}

#[test]
fn uses_natural_case_conditions_and_aligns_matrix_columns() {
    assert_eq!(
        inline(r"\begin{cases}a & x<0 \\ b & \text{if }x=0 \\ c & \text{otherwise}\end{cases}")
            .as_deref(),
        Some("⎧ a if x < 0\n⎨ b if x = 0\n⎩ c otherwise")
    );
    assert_eq!(
        inline(r"\begin{pmatrix}1&200\\3000&4\end{pmatrix}").as_deref(),
        Some("⎛ 1    │ 200 ⎞\n⎝ 3000 │ 4   ⎠")
    );
}

#[test]
fn composes_matrices_with_fractions_and_adjacent_matrices() {
    assert_eq!(
        display(
            "R\\left(\\frac{\\pi}{4}\\right)\n=\n\\begin{pmatrix}\n\\frac{\\sqrt{2}}{2} & -\\frac{\\sqrt{2}}{2}\\\\\n\\frac{\\sqrt{2}}{2} & \\frac{\\sqrt{2}}{2}\n\\end{pmatrix}."
        )
        .as_deref(),
        Some("   π\nR( ─ ) = ⎛ (√2)/2 │ -(√2)/2 ⎞\n   4     ⎝ (√2)/2 │ (√2)/2  ⎠.")
    );
    assert_eq!(
        display(
            "\\mathbf w\n=\nR\\left(\\frac{\\pi}{4}\\right)\n\\begin{pmatrix}1\\\\0\\end{pmatrix}\n=\n\\begin{pmatrix}\\frac{\\sqrt{2}}{2}\\\\\\frac{\\sqrt{2}}{2}\\end{pmatrix}."
        )
        .as_deref(),
        Some("       π\nw = R( ─ ) ⎛ 1 ⎞ = ⎛ (√2)/2 ⎞\n       4   ⎝ 0 ⎠   ⎝ (√2)/2 ⎠.")
    );
    assert_eq!(
        display(
            r"A\mathbf e_1=\begin{pmatrix}\pi\\0\end{pmatrix},\qquad A\mathbf e_2=\begin{pmatrix}0\\\frac{1}{\pi}\end{pmatrix}."
        )
        .as_deref(),
        Some("Ae₁ = ⎛ π ⎞, Ae₂ = ⎛ 0   ⎞\n      ⎝ 0 ⎠        ⎝ 1/π ⎠.")
    );
    assert_eq!(
        display(r"\sum_{i=0}^n x_i=\begin{pmatrix}a&b\\c&d\end{pmatrix}.").as_deref(),
        Some(" n\n ∑  xᵢ = ⎛ a │ b ⎞\ni=0      ⎝ c │ d ⎠.")
    );
}

#[test]
fn normalizes_relation_multiplication_and_named_operator_spacing() {
    for source in ["x=y", "x =y", "x=\ny", "x\n=\ny"] {
        assert_eq!(inline(source).as_deref(), Some("x = y"));
    }
    assert_eq!(inline("x_{i=0}").as_deref(), Some("xᵢ₌₀"));
    assert_eq!(inline(r"x\neq0").as_deref(), Some("x ≠ 0"));
    assert_eq!(inline(r"A\to B").as_deref(), Some("A → B"));
    assert_eq!(inline(r"\pi\cdot\frac{1}{\pi}").as_deref(), Some("π · 1/π"));
    assert_eq!(inline(r"\sin\theta").as_deref(), Some("sin θ"));
    assert_eq!(inline(r"\sin^2 x").as_deref(), Some("sin² x"));
    assert_eq!(inline(r"-\sin\theta").as_deref(), Some("-sin θ"));
    assert_eq!(inline(r"i\sin\theta").as_deref(), Some("i sin θ"));
    assert_eq!(inline(r"\det(A)").as_deref(), Some("det(A)"));
}

#[test]
fn treats_a_backslash_followed_by_a_line_ending_as_control_space() {
    let source = "\\boxed{\n(1,1,1),\\ (1,1,2),\\ (1,2,5),\\ (1,5,13),\\ (2,5,29),\\\n(1,13,34),\\ (1,34,89)\n}.";
    assert_eq!(
        display(source).as_deref(),
        Some("[(1,1,1), (1,1,2), (1,2,5), (1,5,13), (2,5,29), (1,13,34), (1,34,89)].")
    );
    assert_eq!(inline("a\\\r\nb").as_deref(), Some("a b"));
}

#[test]
fn stacks_operator_limits_in_display_mode() {
    assert_eq!(
        display(r"\sum_{i=0}^n x_i").as_deref(),
        Some(" n\n ∑  xᵢ\ni=0")
    );
    assert_eq!(
        display(r"\min_{x\in X} f(x)").as_deref(),
        Some("min f(x)\nx∈X")
    );
    assert_eq!(
        display(r"\operatorname*{arg\,max}_{x\in X} f(x)").as_deref(),
        Some("arg max f(x)\n  x∈X")
    );
    assert_eq!(
        display(r"\int\nolimits_0^1 f(x)\,dx").as_deref(),
        Some("∫₀¹ f(x) dx")
    );
    assert_eq!(
        display(r"\int\limits_0^1 f(x)\,dx").as_deref(),
        Some("1\n∫ f(x) dx\n0")
    );
}

#[test]
fn uses_the_middle_brace_for_intermediate_case_rows() {
    assert_eq!(
        inline(r"\begin{cases}a & x<0 \\ b & x=0 \\ c & x>0\end{cases}").as_deref(),
        Some("⎧ a if x < 0\n⎨ b if x = 0\n⎩ c if x > 0")
    );
}

#[test]
fn stacks_fractions_in_display_mode() {
    assert_eq!(
        display(r"x=\frac{-b\pm\sqrt{b^2-4ac}}{2a}").as_deref(),
        Some("    -b±√(b²-4ac)\nx = ────────────\n         2a")
    );
    assert_eq!(
        display(r"\frac{x^2+1}{x-1}").as_deref(),
        Some("x²+1\n────\nx-1")
    );
    assert_eq!(display("\\frac{1}\n{2}").as_deref(), Some("1\n─\n2"));
}

#[test]
fn keeps_nested_display_fractions_linear() {
    let cases = [
        (
            r"\frac{\frac{x^2+1}{x-1}-\frac{2x}{x+1}}{\frac{x}{x^2-1}}",
            "(x²+1)/(x-1)-2x/(x+1)\n─────────────────────\n      x/(x²-1)",
        ),
        (
            r"\lim_{x\to 0}\frac{\frac{\sin x}{x}-1}{\frac{e^x-1}{x}-1}=0",
            "     (sin x)/x-1\nlim  ─────────── = 0\nx→0  (eˣ-1)/x-1",
        ),
        (
            r"\frac{1+\frac{1}{1+\frac{1}{x}}}{1-\frac{1}{1-\frac{1}{x}}}",
            "1+1/(1+1/x)\n───────────\n1-1/(1-1/x)",
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(display(source).as_deref(), Some(expected), "{source}");
    }
}

#[test]
fn keeps_fractions_linear_in_scripts_and_text_style_fractions() {
    assert_eq!(display(r"e^{\frac{1}{2}}").as_deref(), Some("e^(1/2)"));
    assert_eq!(display(r"\tfrac{1}{2}").as_deref(), Some("1/2"));
}

#[test]
fn returns_none_for_unsupported_commands() {
    assert_eq!(inline(r"x + \unknown{y}"), None);
}

#[test]
fn returns_none_for_malformed_groups_and_environments() {
    for source in [r"\frac{1}{x", "x}", r"\begin{matrix}1 & 2", "x\\"] {
        assert_eq!(inline(source), None, "{source}");
    }
}
