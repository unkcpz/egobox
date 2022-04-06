use crate::errors::{MoeError, Result};
use egobox_gp::{correlation_models::*, mean_models::*, GaussianProcess, GpParams, GpValidParams};
use linfa::prelude::{Dataset, Fit};
use ndarray::{Array2, ArrayView2};
use paste::paste;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;

/// A trait for GP surrogate parameters.
pub trait GpSurrogateParams {
    /// Set initial theta
    fn initial_theta(&mut self, theta: Vec<f64>);
    /// Set number of PLS components
    fn kpls_dim(&mut self, kpls_dim: Option<usize>);
    /// Set the nugget parameter to improve numerical stability
    fn nugget(&mut self, nugget: f64);
    /// Train the surrogate
    fn fit(&self, x: &Array2<f64>, y: &Array2<f64>) -> Result<Box<dyn GpSurrogate>>;
}

/// A trait for GP surrogate.
pub trait GpSurrogate: std::fmt::Display + std::fmt::Debug {
    /// Predict output values at n points given as (n, xdim) matrix.
    fn predict_values(&self, x: &ArrayView2<f64>) -> egobox_gp::Result<Array2<f64>>;
    /// Predict variance values at n points given as (n, xdim) matrix.
    fn predict_variances(&self, x: &ArrayView2<f64>) -> egobox_gp::Result<Array2<f64>>;
    /// Save GP model in given file.
    fn save(&self, path: &str) -> Result<()>;
}

macro_rules! declare_surrogate {
    ($regr:ident, $corr:ident) => {
        paste! {

            /// GP Surrogate parameters with given mean and correlation models. See [egobox_gp::GpParams]
            #[derive(Clone)]
            pub struct [<Gp $regr $corr SurrogateParams>](
                GpParams<f64, [<$regr Mean>], [<$corr Corr>]>,
            );

            impl [<Gp $regr $corr SurrogateParams>] {
                /// Constructor
                pub fn new(gp_params: GpParams<f64, [<$regr Mean>], [<$corr Corr>]>) -> [<Gp $regr $corr SurrogateParams>] {
                    [<Gp $regr $corr SurrogateParams>](gp_params)
                }
            }

            impl GpSurrogateParams for [<Gp $regr $corr SurrogateParams>] {
                fn initial_theta(&mut self, theta: Vec<f64>) {
                    self.0 = self.0.clone().initial_theta(Some(theta));
                }

                fn kpls_dim(&mut self, kpls_dim: Option<usize>) {
                    self.0 = self.0.clone().kpls_dim(kpls_dim);
                }

                fn nugget(&mut self, nugget: f64) {
                    self.0 = self.0.clone().nugget(nugget);
                }

                fn fit(
                    &self,
                    x: &Array2<f64>,
                    y: &Array2<f64>,
                ) -> Result<Box<dyn GpSurrogate>> {
                    Ok(Box::new([<Gp $regr $corr Surrogate>](
                        self.0.clone().fit(&Dataset::new(x.to_owned(), y.to_owned()))?,
                    )))
                }
            }

            /// GP surrogate with given mean and correlation models. See [egobox_gp::GaussianProcess]
            #[derive(Clone, Debug, Serialize, Deserialize)]
            pub struct [<Gp $regr $corr Surrogate>](
                pub GaussianProcess<f64, [<$regr Mean>], [<$corr Corr>]>,
            );

            impl GpSurrogate for [<Gp $regr $corr Surrogate>] {
                fn predict_values(&self, x: &ArrayView2<f64>) -> egobox_gp::Result<Array2<f64>> {
                    self.0.predict_values(x)
                }
                fn predict_variances(&self, x: &ArrayView2<f64>) -> egobox_gp::Result<Array2<f64>> {
                    self.0.predict_variances(x)
                }
                fn save(&self, path: &str) -> Result<()> {
                    let mut file = fs::File::create(path).unwrap();
                    let bytes = match serde_json::to_string(self) {
                        Ok(b) => b,
                        Err(err) => return Err(MoeError::SaveError(err))
                    };
                    file.write_all(bytes.as_bytes())?;
                    Ok(())
                }
            }

            impl std::fmt::Display for [<Gp $regr $corr Surrogate>] {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "{}_{}{}", stringify!($regr), stringify!($corr),
                        match self.0.kpls_dim() {
                            None => String::from(""),
                            Some(dim) => format!("_PLS({})", dim),
                        }
                    )
                }
            }
        }
    };
}

declare_surrogate!(Constant, SquaredExponential);
declare_surrogate!(Constant, AbsoluteExponential);
declare_surrogate!(Constant, Matern32);
declare_surrogate!(Constant, Matern52);
declare_surrogate!(Linear, SquaredExponential);
declare_surrogate!(Linear, AbsoluteExponential);
declare_surrogate!(Linear, Matern32);
declare_surrogate!(Linear, Matern52);
declare_surrogate!(Quadratic, SquaredExponential);
declare_surrogate!(Quadratic, AbsoluteExponential);
declare_surrogate!(Quadratic, Matern32);
declare_surrogate!(Quadratic, Matern52);

/// Create a GP with given regression and correlation models.
#[macro_export]
macro_rules! make_gp_params {
    ($regr:ident, $corr:ident) => {
        paste! {
            GaussianProcess::<f64, [<$regr Mean>], [<$corr Corr>] >::params(
                [<$regr Mean>]::default(),
                [<$corr Corr>]::default(),
            )
        }
    };
}

/// Create GP surrogate parameters with given regression and correlation models.
#[macro_export]
macro_rules! make_surrogate_params {
    ($regr:ident, $corr:ident) => {
        paste! {
            Box::new([<Gp $regr $corr SurrogateParams>]::new(
                make_gp_params!($regr, $corr))
            )
        }
    };
}

/// Create GP surrogate
#[macro_export]
macro_rules! make_surrogate {
    ($regr:ident, $corr:ident, $data:ident) => {
        paste! {
            Box::new(
                [<Gp $regr $corr Surrogate>](
                    GpValidParams::<f64, [<$regr Mean>], [<$corr Corr>]>::from(
                        [<$regr Mean>](), [<$corr Corr>](),
                        serde_json::from_value(serde_json::json!($data["theta"])).unwrap(),
                        serde_json::from_value(serde_json::json!($data["inner_params"])).unwrap(),
                        serde_json::from_value(serde_json::json!($data["w_star"])).unwrap(),
                        serde_json::from_value(serde_json::json!($data["xtrain"])).unwrap(),
                        serde_json::from_value(serde_json::json!($data["ytrain"])).unwrap()
                    )?)
                ) as Box<dyn GpSurrogate>
        }
    };
}

/// Load GP surrogate from given file.
pub fn load(path: &str) -> Result<Box<dyn GpSurrogate>> {
    let data = fs::read_to_string(path)?;
    let data: serde_json::Value = serde_json::from_str(&data)?;
    let gp_kind = format!(
        "{}_{}",
        data["mean"].as_str().unwrap(),
        data["corr"].as_str().unwrap()
    );
    match gp_kind.as_str() {
        "Constant_SquaredExponential" => Ok(make_surrogate!(Constant, SquaredExponential, data)),
        "Constant_AbsoluteExponential" => Ok(make_surrogate!(Constant, AbsoluteExponential, data)),
        "Constant_Matern32" => Ok(make_surrogate!(Constant, Matern32, data)),
        "Constant_Matern52" => Ok(make_surrogate!(Constant, Matern52, data)),
        "Linear_SquaredExponential" => Ok(make_surrogate!(Linear, SquaredExponential, data)),
        "Linear_AbsoluteExponential" => Ok(make_surrogate!(Linear, AbsoluteExponential, data)),
        "Linear_Matern32" => Ok(make_surrogate!(Linear, Matern32, data)),
        "Linear_Matern52" => Ok(make_surrogate!(Linear, Matern52, data)),
        "Quadratic_SquaredExponential" => Ok(make_surrogate!(Quadratic, SquaredExponential, data)),
        "Quadratic_AbsoluteExponential" => {
            Ok(make_surrogate!(Quadratic, AbsoluteExponential, data))
        }
        "Quadratic_Matern32" => Ok(make_surrogate!(Quadratic, Matern32, data)),
        "Quadratic_Matern52" => Ok(make_surrogate!(Quadratic, Matern52, data)),
        _ => Err(MoeError::LoadError(format!(
            "Bad mean or kernel values: {}",
            gp_kind
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use egobox_doe::{Lhs, SamplingMethod};
    use ndarray::array;
    use ndarray_linalg::Norm;
    use ndarray_stats::DeviationExt;

    fn xsinx(x: &Array2<f64>) -> Array2<f64> {
        (x - 3.5) * ((x - 3.5) / std::f64::consts::PI).mapv(|v| v.sin())
    }

    #[test]
    fn test_save_load() {
        let xlimits = array![[0., 25.]];
        let xt = Lhs::new(&xlimits).sample(10);
        let yt = xsinx(&xt);
        let gp = make_surrogate_params!(Constant, SquaredExponential)
            .fit(&xt, &yt)
            .expect("GP fit error");
        gp.save("save_gp.json").expect("GP not saved");
        let gp = load("save_gp.json").expect("GP not loaded");
        let xv = Lhs::new(&xlimits).sample(20);
        let yv = xsinx(&xv);
        let ytest = gp.predict_values(&xv.view()).unwrap();
        let err = ytest.l2_dist(&yv).unwrap() / yv.norm_l2();
        assert_abs_diff_eq!(err, 0., epsilon = 2e-1);
    }

    #[test]
    fn test_load_fail() {
        let gp = load("notfound.json");
        assert!(gp.is_err());
    }
}
